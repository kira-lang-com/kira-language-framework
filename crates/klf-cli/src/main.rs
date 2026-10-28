//! `klf`: the KLF toolchain command line.
//!
//! `klf run` executes a compiled module on the VM or native backend. `klf build`
//! can emit canonical KIR, a host-native binary, or a native language frontend.

mod abi;
mod clean;
mod dist;
mod editor;
mod ffi;
mod format;
mod kir;
mod ksir;
mod lsp;
mod pipeline;
mod trap;
mod push;
mod ridl;
mod setup;
mod test;

use klf_diag::{err, out, raw, raw_err};
use klf_driver::{Backend, LanguageDefinition};
use klf_services::Session;
use klf_target::Target;
use klf_vm::Outcome;
use std::path::Path;
use std::process::ExitCode;

fn default_target() -> &'static str {
    klf_driver::host_triple()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => command_run(&args[1..]),
        Some("build") => command_build(&args[1..]),
        Some("check") => command_check(&args[1..]),
        Some("test") => test::command(&args[1..]),
        Some("push") => push::command(&args[1..]),
        Some("dist") => dist::command(&args[1..]),
        Some("setup") => setup::command(&args[1..]),
        Some("editor") => editor::command(&args[1..]),
        Some("clean") => clean::command(&args[1..]),
        Some("kir") => kir::command(&args[1..]),
        Some("ksir") => ksir::command(&args[1..]),
        Some("lsp") => lsp::command(&args[1..]),
        Some("format") => format::command(&args[1..]),
        Some("ridl") => ridl::command(&args[1..]),
        Some("abi") => abi::command(&args[1..]),
        Some("ffi") => ffi::command(&args[1..]),
        Some("--help" | "-h") | None => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("klf: unknown command `{other}`");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

/// The command list, on stdout: `--help` is a result the caller asked for, not
/// a failure over a mistyped word.
fn print_usage() {
    out!(
        "usage:\n  klf run <file.klfk|.klfd|.calc|.kir> [--entry NAME] [--input TEXT] [--backend vm|native] [--target TRIPLE] [--timings] [-- ARGS...]\n  klf build <file.klfk|.klfd|.calc|.kir> [--emit kir|bin] [--backend native] [--target TRIPLE] [--profile debug|release] [--timings] [-o OUT]\n  klf build <definition-package-dir> [--target TRIPLE] [--profile debug|release] [-o NAME] [--timings]   (native frontend)
  klf build <definition-package-dir> [--language PKG]...   (link other shader languages in)\n  klf build <definition-package-dir> --out DIR [--targets T1,T2] [--profile debug|release] [--timings]   (driver, std and sysroot)\n  klf check <definition-package-dir> [--timings]\n  klf format <definition-package-dir> [files] [--check] [--width N] [--timings]\n  klf test <definition-package-dir> [--corpus NAME] [--timings]\n  klf push <file> <host:port> [--entry NAME]   (hot module push, section 17.5)\n  klf clean\n  klf dist --out DIR [--targets T1,T2] [--timings]\n  klf setup [--llvm]   (install or update klf and its LLVM from their releases)\n  klf kir verify|dump <file.kir>\n  klf ksir verify|dump <file.ksir>\n  klf lsp   (language server for .klfd, .kir and .ridl files)\n  klf editor gen <definition-package-dir> --out DIR [--editor vscode,zed,neovim,helix,emacs|all] [--client FILE.js]\n  klf editor gen --kernel --out DIR [--editor ...]\n  klf ridl check|emit|layout|mangle <file.ridl>\n  klf ridl bindgen [--lang rust|swift] <file.ridl>\n  klf abi freeze|apply|retire <file.ridl> [uuid]\n  klf ffi gen <header.h> [--library NAME] [-o OUT.klffi]\n  klf ffi emit|decls <file.klffi>\n\nThe language is chosen by extension: .calc is the toy calculator, everything\nelse is KLF Kira. --input supplies the source buffer to a definition.\n\nA compiling command animates its phases on stderr and takes them down when it returns. --timings turns that off and pushes every phase to stdout as one line with the milliseconds it took. --profile release runs the KIR optimizer over what is built; debug, the default, runs only the mandatory passes. A check prints `Check: Ok` and a build prints `Built: Ok <artifact>`, or the word after the colon is `Failed`."
    );
}

fn command_check(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        return fail("usage: klf check <definition-package-dir> [--timings]");
    };
    if args.len() > 2 || args.iter().skip(1).any(|a| a != "--timings") {
        return fail("klf check: expected one definition package directory");
    }
    let timings = args.iter().any(|argument| argument == "--timings");
    let title = format!("Checking {root}");
    let mut finish = klf_diag::status::begin(&title, timings);
    let target = match resolve_target(default_target()) {
        Ok(target) => target,
        Err(message) => return fail(&message),
    };
    let mut session = Session::new(target);
    let definition = LanguageDefinition::load(&mut session, Path::new(root), Backend::Vm);
    if session.error_count() > 0 {
        finish.end();
        raw_err!("{}", pipeline::render(&session));
        out!("Check: Failed");
        return ExitCode::FAILURE;
    }
    if definition.is_none() {
        finish.end();
        out!("Check: Failed");
        return fail("klf check: definition package did not compile");
    }
    finish.end();
    out!("Check: Ok");
    ExitCode::SUCCESS
}

struct Options {
    file: Option<String>,
    entry: String,
    target: String,
    emit: String,
    backend: Option<String>,
    input: Option<String>,
    output: Option<String>,
    out: Option<String>,
    targets: Option<String>,
    /// The other shader languages to link into a frontend, each named by its
    /// own definition package. `klf build` links what it is told to, not
    /// what it finds beside itself.
    languages: Vec<String>,
    call_args: Vec<u64>,
    /// Whether to print what each phase took in milliseconds instead of
    /// animating the status surface.
    timings: bool,
    profile: klf_driver::Profile,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            file: None,
            entry: "main".to_string(),
            target: default_target().to_string(),
            emit: "kir".to_string(),
            backend: None,
            input: None,
            output: None,
            out: None,
            targets: None,
            languages: Vec::new(),
            call_args: Vec::new(),
            timings: false,
            profile: klf_driver::Profile::Debug,
        }
    }
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--entry" => options.entry = take(args, &mut index)?,
            "--target" => options.target = take(args, &mut index)?,
            "--emit" => options.emit = take(args, &mut index)?,
            "--backend" => options.backend = Some(take(args, &mut index)?),
            "--input" => options.input = Some(take(args, &mut index)?),
            "-o" => options.output = Some(take(args, &mut index)?),
            "--out" => options.out = Some(take(args, &mut index)?),
            "--targets" => options.targets = Some(take(args, &mut index)?),
            "--language" => options.languages.push(take(args, &mut index)?),
            "--timings" => options.timings = true,
            "--profile" => {
                let name = take(args, &mut index)?;
                options.profile = klf_driver::Profile::named(&name)
                    .ok_or_else(|| format!("`--profile` is `debug` or `release`, not `{name}`"))?;
            }
            "--" => {
                for raw in &args[index + 1..] {
                    options.call_args.push(
                        raw.parse()
                            .map_err(|_| format!("argument `{raw}` is not an integer"))?,
                    );
                }
                break;
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag `{flag}`")),
            value => {
                if options.file.is_some() {
                    return Err(format!("unexpected argument `{value}`"));
                }
                options.file = Some(value.to_string());
            }
        }
        index += 1;
    }
    Ok(options)
}

fn is_kir(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("kir"))
}

fn take(args: &[String], index: &mut usize) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("`{}` needs a value", args[*index - 1]))
}

fn resolve_target(triple: &str) -> Result<Target, String> {
    triple
        .parse()
        .map_err(|_| format!("invalid target triple `{triple}`"))
}

fn read_input(options: &Options) -> Result<(String, String), String> {
    let path = options.file.as_ref().ok_or("expected a source file")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    Ok((path.clone(), text))
}

fn module_name(file: &str) -> String {
    std::path::Path::new(file)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Main")
        .to_string()
}

fn command_run(args: &[String]) -> ExitCode {
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(message) => return fail(&message),
    };
    let title = format!("Running {}", options.file.as_deref().unwrap_or("<input>"));
    let mut finish = klf_diag::status::begin(&title, options.timings);
    let (path, source) = match read_input(&options) {
        Ok(pair) => pair,
        Err(message) => return fail(&message),
    };
    let target = match resolve_target(&options.target) {
        Ok(target) => target,
        Err(message) => return fail(&message),
    };
    let backend = match options.backend.as_deref().unwrap_or("vm") {
        "vm" => Backend::Vm,
        "native" => Backend::Native,
        other => {
            return fail(&format!(
                "unknown backend `{other}` (expected `vm` or `native`)"
            ));
        }
    };
    if is_kir(&path) {
        let module = match pipeline::parse_kir(&source, &path) {
            Ok(module) => module,
            Err(message) => return fail(&message),
        };
        finish.end();
        return match pipeline::run(&module, &options.entry, &options.call_args, backend) {
            Ok(outcome) => report_outcome(&outcome),
            Err(message) => fail(&message),
        };
    }
    let mut program =
        match pipeline::compile_with_session(&source, &path, &module_name(&path), target) {
            Ok(program) => program,
            Err(error) => {
                raw_err!("{}", error.rendered);
                return ExitCode::FAILURE;
            }
        };
    let warnings = pipeline::render_runtime_diagnostics(&program.session);
    if !warnings.is_empty() {
        raw_err!("{warnings}");
    }
    let kept = program.session.runtime_diagnostics().len();
    let result = if let Some(input) = options.input.as_deref() {
        let result = pipeline::run_compiled_with_source(
            &mut program,
            &options.entry,
            &options.call_args,
            input,
            backend,
        );
        let diagnostics = pipeline::render_runtime_diagnostics_from(&program.session, kept);
        if !diagnostics.is_empty() {
            raw_err!("{diagnostics}");
        }
        result
    } else {
        pipeline::run(&program.module, &options.entry, &options.call_args, backend)
    };
    finish.end();
    match result {
        Ok(outcome) => report_outcome(&outcome),
        Err(message) => fail(&message),
    }
}

fn command_build(args: &[String]) -> ExitCode {
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(message) => return fail(&message),
    };
    // A definition package is a directory, and the only thing `klf build` can
    // make of one is the compiler driver it describes (section 5.2).
    let is_package = options
        .file
        .as_deref()
        .is_some_and(|path| std::path::Path::new(path).is_dir());
    let subject = options.file.as_deref().unwrap_or("<input>");
    if options.out.is_some() {
        let mut finish =
            klf_diag::status::begin(&format!("Building sysroot {subject}"), options.timings);
        return build_sysroot(&options, &mut finish);
    }
    if options.emit == "frontend" || is_package {
        let mut finish = klf_diag::status::begin(&format!("Building {subject}"), options.timings);
        return build_frontend(&options, &mut finish);
    }
    build_module(&options)
}

/// `klf build <file>`: compile one source file to a module and emit it as KIR
/// or as a host-native binary.
#[allow(clippy::too_many_lines)]
fn build_module(options: &Options) -> ExitCode {
    let (path, source) = match read_input(options) {
        Ok(pair) => pair,
        Err(message) => return fail(&message),
    };
    let target = match resolve_target(&options.target) {
        Ok(target) => target,
        Err(message) => return fail(&message),
    };
    if options.emit == "bin" && target.triple() != default_target() {
        return fail(&unsupported_native_target(target.triple()));
    }
    // KIR to stdout is a payload: nothing else may share that stream, so that
    // form gets no status lines at all rather than lines a reader has to strip
    // back out of the module.
    let payload_to_stdout = options.emit == "kir" && options.output.is_none();
    if options.emit == "bin" {
        if options
            .backend
            .as_deref()
            .is_some_and(|backend| backend != "native")
        {
            return fail("`--emit bin` currently requires the native (Cranelift) backend");
        }
        if options.output.is_none() {
            return fail("`--emit bin` requires `-o OUT`");
        }
    } else if options.emit != "kir" {
        return fail(&format!(
            "`--emit {}` is not supported yet (use `kir`, `bin`, or `frontend`)",
            options.emit
        ));
    }
    let subject = options.file.as_deref().unwrap_or("<input>");
    let mut finish = if payload_to_stdout {
        klf_diag::status::silent()
    } else {
        klf_diag::status::begin(&format!("Building {subject}"), options.timings)
    };
    let mut module = if is_kir(&path) {
        match pipeline::parse_kir(&source, &path) {
            Ok(module) => module,
            Err(message) => return fail(&message),
        }
    } else {
        match pipeline::compile(&source, &path, &module_name(&path), target) {
            Ok(module) => module,
            Err(error) => {
                finish.end();
                raw_err!("{}", error.rendered);
                out!("Built: Failed");
                return ExitCode::FAILURE;
            }
        }
    };
    if options.emit == "bin" {
        if module.target.triple() != default_target() {
            finish.end();
            return fail(&unsupported_native_target(module.target.triple()));
        }
        let output = options
            .output
            .as_deref()
            .expect("output checked before compilation");
        let built = klf_driver::build_native(
            &module,
            &options.entry,
            &options.call_args,
            std::path::Path::new(output),
            options.profile,
        );
        finish.end();
        return match built {
            Ok(()) => {
                out!("Built: Ok {output}");
                ExitCode::SUCCESS
            }
            Err(message) => {
                out!("Built: Failed");
                fail(&message)
            }
        };
    }
    if options.profile == klf_driver::Profile::Release {
        module = klf_kir_passes::monomorphize(&module);
        let backend = match options.backend.as_deref() {
            Some("llvm") => klf_kir_passes::Backend::Llvm,
            Some("vm") => klf_kir_passes::Backend::Vm,
            _ => klf_kir_passes::Backend::Cranelift,
        };
        klf_kir_passes::optimize(&mut module, options.profile.level(), backend);
    }
    let text = module.to_string();
    if let Some(out) = &options.output {
        let written = std::fs::write(out, text);
        finish.end();
        if let Err(e) = written {
            fail(&format!("cannot write `{out}`: {e}"))
        } else {
            out!("Built: Ok {out}");
            ExitCode::SUCCESS
        }
    } else {
        raw!("{text}");
        ExitCode::SUCCESS
    }
}

/// `klf build <package> --emit frontend`: link the definition package into the
/// native compiler driver it describes (section 5.2).
fn build_frontend(options: &Options, finish: &mut klf_diag::status::Finish) -> ExitCode {
    let Some(path) = options.file.as_ref() else {
        return fail("`klf build --emit frontend` needs a definition package");
    };
    let target = match resolve_target(&options.target) {
        Ok(target) => target,
        Err(message) => return fail(&message),
    };
    let mut session = klf_services::Session::new(target.clone());
    let output = options.output.as_ref().map(std::path::Path::new);
    let languages: Vec<&std::path::Path> =
        options.languages.iter().map(std::path::Path::new).collect();
    match klf_driver::build_frontend(
        &mut session,
        std::path::Path::new(path),
        output,
        &target,
        &languages,
        options.profile,
    ) {
        Ok(built) => {
            if session.error_count() > 0 {
                finish.end();
                raw_err!("{}", pipeline::render(&session));
                out!("Built: Failed");
                return ExitCode::FAILURE;
            }
            finish.end();
            out!("Built: Ok {}", built.binary.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            finish.end();
            if session.error_count() > 0 {
                raw_err!("{}", pipeline::render(&session));
            }
            out!("Built: Failed");
            fail(&message)
        }
    }
}

/// `klf build <package> --out <dir>`: the driver, the std compiled with it,
/// and the sysroot around them (sections 5.3 and 5.4).
fn build_sysroot(options: &Options, finish: &mut klf_diag::status::Finish) -> ExitCode {
    let (Some(path), Some(out)) = (options.file.as_ref(), options.out.as_ref()) else {
        return fail("`klf build --out DIR` needs a definition package");
    };
    let names = options.targets.as_deref().unwrap_or(&options.target);
    let mut targets = Vec::new();
    for name in names.split(',').filter(|name| !name.is_empty()) {
        match resolve_target(name) {
            Ok(target) => targets.push(target),
            Err(message) => return fail(&message),
        }
    }
    let Some(first) = targets.first() else {
        return fail("`--targets` needs at least one triple");
    };
    let mut session = klf_services::Session::new(first.clone());
    let languages: Vec<&std::path::Path> =
        options.languages.iter().map(std::path::Path::new).collect();
    match klf_driver::build_sysroot(
        &mut session,
        std::path::Path::new(path),
        std::path::Path::new(out),
        &targets,
        &languages,
        options.profile,
    ) {
        Ok(built) => {
            finish.end();
            out!("Built: Ok {}", built.root.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            finish.end();
            if session.error_count() > 0 {
                raw_err!("{}", pipeline::render(&session));
            }
            out!("Built: Failed");
            fail(&message)
        }
    }
}

/// Why a native binary cannot be made for the triple asked for: only the host
/// triple has a provisioned toolchain.
fn unsupported_native_target(triple: &str) -> String {
    format!(
        "native binary target `{triple}` is unsupported; current toolchain targets `{}`",
        default_target()
    )
}

fn report_outcome(outcome: &Outcome) -> ExitCode {
    match outcome {
        Outcome::Return(Some(value)) => {
            out!("{value}");
            ExitCode::SUCCESS
        }
        Outcome::Return(None) => ExitCode::SUCCESS,
        Outcome::Fail(value) => {
            err!("fail: {value}");
            ExitCode::FAILURE
        }
        Outcome::Trap(message) => {
            err!("{}", trap::line(message));
            ExitCode::FAILURE
        }
    }
}

fn fail(message: &str) -> ExitCode {
    err!("klf: {message}");
    ExitCode::FAILURE
}
