//! `libklfvm.a` (section 5.1): the VM core behind C entry points, so a
//! sysroot can link its bytecode runner, `bin/<driver>-run`, with the bundled
//! clang and no Rust toolchain. It contains the native runtime as well, so
//! the runner can host a native std that bytecode calls into.
//!
//! The runner reports like a native program: service output as `IO <value>`
//! lines, then one `RET <value>`, `RET void`, `FAIL <value>` or `TRAP <code>`
//! line, and exit status 0. A usage or load error exits with status 2.

#![allow(unsafe_code)]

mod live;

// The native runtime is linked in whole: std code a runner loads calls it,
// and one archive keeps a single Rust runtime in the runner.
pub use klf_native_rt;
use klf_vm::{Capabilities, Instance, Outcome, Program};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write as _;

/// Loads a `.klbc` image and runs `entry` with `arguments`.
///
/// # Errors
/// Returns a message when the image does not decode or has no such entry.
pub fn run_image(image: &[u8], entry: &str, arguments: &[u64]) -> Result<Outcome, String> {
    run_image_from(image, std::path::Path::new("."), entry, arguments)
}

/// [`run_image`] for an image whose native libraries sit in `base`.
///
/// # Errors
/// Returns a message when the image does not decode, a library it imports
/// does not load, or it has no such entry.
pub fn run_image_from(
    image: &[u8],
    base: &std::path::Path,
    entry: &str,
    arguments: &[u64],
) -> Result<Outcome, String> {
    let program: Program = klf_vm::decode(image).map_err(|error| format!("{error:?}"))?;
    let index = program
        .function_index(entry)
        .ok_or_else(|| format!("no function named `{entry}`"))?;
    let instance = Instance::load(program, base, capabilities())?;
    Ok(match instance.initialize() {
        Outcome::Return(_) => instance.run(index, arguments),
        ended => ended,
    })
}

/// The kernel services with output written as the native runtime writes it.
fn capabilities() -> Capabilities {
    let (mut capabilities, _) = klf_vm::kernel_services();
    capabilities.register_with_memory("klfsvc_io_write", |arguments, memory| {
        let [address, length] = arguments else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*length)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if count > 0 && !memory.read_buffer(*address, &mut bytes) {
            return u64::MAX;
        }
        let mut output = std::io::stdout().lock();
        match output.write_all(&bytes).and_then(|()| output.flush()) {
            Ok(()) => *length,
            Err(_) => u64::MAX,
        }
    });
    capabilities.register("klfsvc_io_put", |arguments| {
        let byte = arguments.first().copied().unwrap_or(0) & 0xff;
        println!("IO {byte}");
        byte
    });
    capabilities.register("klfsvc_io_puti", |arguments| {
        let value = i64::from_ne_bytes(arguments.first().copied().unwrap_or(0).to_ne_bytes());
        println!("IO {value}");
        0
    });
    capabilities
}

/// The outcome line a native program prints for the same result.
#[must_use]
pub fn outcome_line(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Return(Some(value)) => format!("RET {value}"),
        Outcome::Return(None) => "RET void".to_owned(),
        Outcome::Fail(value) => format!("FAIL {value}"),
        Outcome::Trap(code) => format!("TRAP {}", klf_diag::report::outcome_line(code)),
    }
}

/// `<driver>-run <file.klbc> [--entry NAME] [ARGUMENT...]`, where every
/// argument is an unsigned 64-bit integer.
///
/// # Safety
/// `argument_values` must hold `argument_count` NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_main(
    argument_count: c_int,
    argument_values: *const *const c_char,
) -> c_int {
    let mut arguments = Vec::new();
    for index in 1..usize::try_from(argument_count).unwrap_or(0) {
        // SAFETY: the caller promises argument_count valid entries.
        let pointer = unsafe { *argument_values.add(index) };
        if !pointer.is_null() {
            // SAFETY: each entry is NUL-terminated.
            arguments.push(
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    match run_command(&arguments) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(message) => {
            eprintln!("error: {message}");
            2
        }
    }
}

/// `<driver>-run <file.klbc> [ARGUMENT...]` for a language whose programs
/// are C processes: `main` gets the image path and the arguments as `argc`
/// and `argv`, and the environment as `envp`; its value is the exit status,
/// and a trap is reported on stderr and aborts, as a native program does.
///
/// # Safety
/// `argument_values` must hold `argument_count` NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_process_main(
    argument_count: c_int,
    argument_values: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise is passed on unchanged.
    unsafe { process_main(argument_count, argument_values, false) }
}

/// [`klfvm_process_main`] for a language whose trap exits with status 1.
///
/// # Safety
/// `argument_values` must hold `argument_count` NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_process_exit_main(
    argument_count: c_int,
    argument_values: *const *const c_char,
) -> c_int {
    // SAFETY: the caller's promise is passed on unchanged.
    unsafe { process_main(argument_count, argument_values, true) }
}

unsafe fn process_main(
    argument_count: c_int,
    argument_values: *const *const c_char,
    trap_exits: bool,
) -> c_int {
    let mut arguments = Vec::new();
    for index in 1..usize::try_from(argument_count).unwrap_or(0) {
        // SAFETY: the caller promises `argument_count` valid entries.
        let pointer = unsafe { *argument_values.add(index) };
        if !pointer.is_null() {
            // SAFETY: each entry is NUL-terminated.
            arguments.push(
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    let Some(path) = arguments.first().cloned() else {
        eprintln!("usage: <driver>-run <file.klbc> [ARGUMENT...]");
        return 2;
    };
    let program = match std::fs::read(&path)
        .map_err(|error| format!("cannot read {path}: {error}"))
        .and_then(|image| klf_vm::decode(&image).map_err(|error| format!("{error:?}")))
    {
        Ok(program) => program,
        Err(message) => {
            eprintln!("error: {message}");
            return 2;
        }
    };
    let image = std::path::absolute(&path).unwrap_or_else(|_| path.clone().into());
    let base = image.parent().unwrap_or(std::path::Path::new("."));
    let Some(entry) = program.function_index("main") else {
        eprintln!("error: no function named `main`");
        return 2;
    };
    let environment: Vec<String> = std::env::vars()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    klf_native_rt::klfrt_process_start(i32::from(trap_exits));
    klf_host_process::set_invoked_as(arguments[0].clone().into_bytes());
    klf_host_process::set_arguments(
        arguments[1..]
            .iter()
            .map(|argument| argument.clone().into_bytes())
            .collect(),
    );
    let instance = match Instance::load(program, base, capabilities()) {
        Ok(instance) => instance,
        Err(message) => {
            eprintln!("error: {message}");
            return 2;
        }
    };
    let outcome = match instance.initialize() {
        Outcome::Return(_) => instance.run_process(entry, &arguments, &environment),
        ended => ended,
    };
    match outcome {
        // The C exit status is the low bits of `main`'s `int`.
        #[allow(clippy::cast_possible_truncation)]
        Outcome::Return(value) => value.unwrap_or(0) as c_int,
        Outcome::Fail(value) => {
            eprintln!("uncaught failure {value}");
            1
        }
        Outcome::Trap(code) => {
            eprint!("{}", trap_text(&code));
            if trap_exits {
                return 1;
            }
            std::process::abort()
        }
    }
}

/// Runs the image a native library was built beside in an instance of its
/// own, bound to that library: the constructor of a RIDL module's library
/// calls it, so a program that links the library, natively or from
/// bytecode, calls the module's entries and they run on its instance
/// (section 17.3). The image is the library's path without its
/// `.natives.so`, `.natives.dylib` or `.natives.dll` suffix. The instance lives as long as
/// the process; an image that cannot load or initialize ends it.
///
/// # Safety
/// `anchor` must be an address inside the library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_attach(anchor: *const c_void) {
    let attached = klf_vm::image_of(anchor as usize)
        .ok_or_else(|| "a library asked for its image from outside any image".to_owned())
        .and_then(|library| {
            let image = [".natives.so", ".natives.dylib", ".natives.dll"]
                .iter()
                .find_map(|suffix| library.strip_suffix(suffix))
                .ok_or_else(|| format!("{library} is not the native library of an image"))?
                .to_owned();
            let program = std::fs::read(&image)
                .map_err(|error| format!("cannot read {image}: {error}"))
                .and_then(|bytes| {
                    klf_vm::decode(&bytes).map_err(|error| format!("{image}: {error:?}"))
                })?;
            let base = std::path::Path::new(&image)
                .parent()
                .unwrap_or(std::path::Path::new("."));
            Instance::load(program, base, capabilities())
        });
    let instance = match attached {
        Ok(instance) => instance,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(2);
        }
    };
    match instance.initialize() {
        Outcome::Return(_) => {
            Box::leak(instance);
        }
        Outcome::Fail(value) => {
            eprintln!("uncaught failure {value}");
            std::process::exit(1);
        }
        Outcome::Trap(code) => {
            eprint!("{}", trap_text(&code));
            std::process::exit(1);
        }
    }
}

fn run_command(arguments: &[String]) -> Result<String, String> {
    let mut path = None;
    let mut entry = "main".to_owned();
    let mut values = Vec::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        if argument == "--entry" {
            entry.clone_from(rest.next().ok_or("`--entry` needs a name")?);
        } else if path.is_none() {
            path = Some(argument.clone());
        } else {
            values.push(
                argument
                    .parse::<u64>()
                    .map_err(|_| format!("argument `{argument}` is not an integer"))?,
            );
        }
    }
    let path = path.ok_or("usage: <driver>-run <file.klbc> [--entry NAME] [ARGUMENT...]")?;
    let image = std::fs::read(&path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let image_path = std::path::absolute(&path).unwrap_or_else(|_| path.clone().into());
    let base = image_path.parent().unwrap_or(std::path::Path::new("."));
    run_image_from(&image, base, &entry, &values).map(|outcome| outcome_line(&outcome))
}

/// Runs `entry` of the `.klbc` image at `image[..length]` with `word_count` words
/// from `words`. Writes the result word to `result` and returns 0 for a
/// return, 1 for a failure (the failure value in `result`), 2 for a trap and
/// 3 when the image cannot run.
///
/// # Safety
/// The pointers must be valid for the given lengths; `entry` must be
/// NUL-terminated and `result` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_run(
    image: *const u8,
    length: usize,
    entry: *const c_char,
    words: *const u64,
    word_count: usize,
    result: *mut u64,
) -> c_int {
    // SAFETY: the caller promises valid pointers and lengths.
    let (image, entry, arguments) = unsafe {
        (
            std::slice::from_raw_parts(image, length),
            CStr::from_ptr(entry).to_string_lossy().into_owned(),
            if word_count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(words, word_count)
            },
        )
    };
    let (status, word) = match run_image(image, &entry, arguments) {
        Ok(Outcome::Return(value)) => (0, value.unwrap_or(0)),
        Ok(Outcome::Fail(value)) => (1, value),
        Ok(Outcome::Trap(_)) => (2, 0),
        Err(_) => (3, 0),
    };
    if !result.is_null() {
        // SAFETY: the caller promises `result` is writable.
        unsafe { result.write(word) };
    }
    status
}

/// The report a trap prints on standard error: the one the compiler
/// rendered for a language check, or what its code means.
fn trap_text(code: &str) -> String {
    if klf_diag::report::report_code(code).is_some() {
        return code.to_owned();
    }
    klf_diag::report::trap_description(code).render()
}
