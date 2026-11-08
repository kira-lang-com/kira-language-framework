//! `--emit run`: build the program as an executable, then run it.
//!
//! A definition with a `runProgram` hook decides how: the hook gets the
//! command that runs the executable and the driver arguments of the build,
//! and its result is the driver's exit status. Without one the executable
//! runs with the arguments after `--`, on the driver's own input and output,
//! and its exit status is the driver's.

use crate::{RUN_PROGRAM_SELECTOR, klffe_phase};
use klf_defsvc::{FrontendMetadata, RunProgramCall, ServiceSink};
use klf_native_rt::services;
use klf_services::EntryConvention;
use std::path::{Path, PathBuf};

/// Where a run's executable is written: `-o` when given, else a place of its
/// own under the KLF cache root, the same for every run of the same inputs so
/// a rebuild reuses the machine code of the last. Also whether the driver
/// removes it after the run, as it does a temporary one.
pub(crate) fn executable_path(
    output: Option<PathBuf>,
    inputs: &[PathBuf],
    triple: &str,
) -> (PathBuf, bool) {
    if let Some(output) = output {
        return (output, false);
    }
    let stem = inputs
        .first()
        .and_then(|input| std::path::absolute(input).ok())
        .and_then(|input| input.file_stem().map(std::ffi::OsStr::to_owned))
        .unwrap_or_else(|| "program".into());
    let Some(root) = klf_target::cache_root() else {
        let directory = std::env::temp_dir().join(format!("klf-run-{}", std::process::id()));
        return (directory.join(stem), true);
    };
    let mut identity = Vec::new();
    for input in inputs {
        let input = std::path::absolute(input).unwrap_or_else(|_| input.clone());
        identity.extend_from_slice(input.as_os_str().as_encoded_bytes());
        identity.push(0);
    }
    identity.extend_from_slice(triple.as_bytes());
    let digest = identity
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        });
    (
        root.join("run").join(format!("{digest:016x}")).join(stem),
        false,
    )
}

/// Makes the directory `executable` is written to.
pub(crate) fn prepare(executable: &Path) -> Result<(), String> {
    let Some(directory) = executable
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
    else {
        return Ok(());
    };
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))
}

/// Runs the built `executable` and answers the driver's exit status.
pub(crate) fn run(
    has_hook: bool,
    executable: &Path,
    program_arguments: &[String],
    driver_arguments: &[String],
) -> Result<i32, String> {
    let program = std::path::absolute(executable).unwrap_or_else(|_| executable.to_owned());
    if has_hook {
        let mut command = vec![program.to_string_lossy().into_owned()];
        command.extend_from_slice(program_arguments);
        return run_hook(&command, driver_arguments);
    }
    let status = std::process::Command::new(&program)
        .args(program_arguments)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", program.display()))?;
    Ok(status.code().unwrap_or(1))
}

pub(crate) fn run_bytecode(
    metadata: &FrontendMetadata,
    triple: &str,
    image: &Path,
    program_arguments: &[String],
    driver_arguments: &[String],
) -> Result<i32, String> {
    let runner = bytecode_runner(triple)?;
    let convention = match metadata.entry {
        EntryConvention::Outcome => "outcome",
        EntryConvention::Process => "process",
        EntryConvention::ProcessExit => "process-exit",
    };
    let command = std::iter::once(runner.to_string_lossy().into_owned())
        .chain(std::iter::once(format!(
            "--klf-entry-convention={convention}"
        )))
        .chain(std::iter::once(image.to_string_lossy().into_owned()))
        .chain(program_arguments.iter().cloned())
        .collect::<Vec<_>>();
    if metadata.declares(klf_defsvc::Hook::RunProgram) {
        return run_hook(&command, driver_arguments);
    }
    let status = std::process::Command::new(&runner)
        .arg(image)
        .args(program_arguments)
        .env("KLF_VM_ENTRY_CONVENTION", convention)
        .status()
        .map_err(|error| format!("cannot run bytecode with {}: {error}", runner.display()))?;
    Ok(status.code().unwrap_or(1))
}

fn bytecode_runner(target: &str) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("KLF_VM_RUNNER") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "KLF_VM_RUNNER does not name a runnable file".into());
    }
    let file = if target.contains("-windows-") {
        "klfvm-run.exe"
    } else {
        "klfvm-run"
    };
    let mut candidates = Vec::new();
    if let Some(root) = crate::output::installed_sysroot() {
        candidates.push(root.join("bin").join(file));
        candidates.push(root.join("platforms").join(target).join(file));
    }
    if let Some(home) = std::env::var_os("KLF_HOME") {
        candidates.push(
            PathBuf::from(home)
                .join("platforms")
                .join(target)
                .join(file),
        );
    } else {
        candidates.extend(
            crate::runtime::installed_distributions()
                .into_iter()
                .map(|home| home.join("platforms").join(target).join(file)),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "no Kira runtime backend runner for `{target}`; install the KLF runtime toolchain"
            )
        })
}

/// Calls the definition's `runProgram` hook through the trampoline's run
/// selector.
fn run_hook(command: &[String], driver_arguments: &[String]) -> Result<i32, String> {
    let mut sink = ServiceSink::new();
    let call = RunProgramCall::register(command, driver_arguments, &mut sink);
    services::install(sink.into_table(), String::new());
    let [command_count, total] = call
        .words()
        .map(|word| i64::try_from(word).unwrap_or(i64::MAX));
    // SAFETY: `klffe_phase` is emitted into this binary with this signature;
    // the run selector calls the generated `runProgram` entry with the two
    // counts.
    let word = unsafe { klffe_phase(RUN_PROGRAM_SELECTOR, command_count, total, 0) };
    let reported = services::uninstall();
    if !reported.is_empty() {
        return Err("runProgram reported a diagnostic".to_owned());
    }
    Ok(RunProgramCall::status(word.cast_unsigned()))
}

/// Removes a temporary executable and the directory made for it.
pub(crate) fn remove(executable: &Path) {
    let _ = std::fs::remove_file(executable);
    if let Some(directory) = executable.parent() {
        let _ = std::fs::remove_dir(directory);
    }
}
