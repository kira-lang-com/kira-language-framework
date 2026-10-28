//! The command line of a generated driver (`KLF-CLI.md` section 5): the
//! kernel's built-in commands, the Language's options on each of them, and the
//! definition's own commands beside them, as one [`klf_args::Command`] tree.

use crate::FrontendMetadata;
use klf_args::{Action, Arg, ArgKind, Arity, Command, Matches, ValueType};
use klf_services::{DriverFlagValue, FileMode, Session};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// What `--emit` accepts.
pub const EMIT_KINDS: [&str; 7] = ["kir", "klbc", "bin", "lib", "shared", "ridl", "run"];

/// What a `build`, `run`, `check` or `format` command line asked for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Invocation {
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub export: Option<String>,
    /// What to write: an [`EMIT_KINDS`] entry, or `check` for nothing.
    pub emit: String,
    pub target: Option<String>,
    pub flag_overrides: HashMap<String, DriverFlagValue>,
    pub file_mode_globs: Vec<(String, String)>,
    /// What follows `--`: the arguments `run` runs the program with.
    pub program_arguments: Vec<String>,
    /// The `<host:port>` `push` sends the package's KIR to (section 17.5).
    pub device: Option<String>,
    /// Whether to print every phase with its milliseconds instead of
    /// animating them (section 5.2.1).
    pub timings: bool,
    /// Where `build` writes every path the compile read, probed or listed, as
    /// a make-style depfile (section 5.5).
    pub depfile: Option<PathBuf>,
    /// Whether an emitted `.ridl` names where each declaration is written
    /// (`KLF-EDITOR.md` section 15.3).
    pub ridl_source: bool,
    /// Where a `shared` build also writes the library's `.ridl` contract,
    /// from the same lowered module: one compile for both.
    pub contract: Option<PathBuf>,
}

fn inputs(help: &str) -> Arg {
    Arg::new("inputs", ArgKind::Positional, ValueType::String)
        .many()
        .value_name("path")
        .help(help)
}

fn timings() -> Arg {
    Arg::new("timings", ArgKind::Flag, ValueType::Bool)
        .help("Print every phase with the milliseconds it took instead of animating them.")
}

fn target() -> Arg {
    Arg::new("target", ArgKind::Option, ValueType::String)
        .value_name("triple")
        .help("Compile for this target triple instead of the driver's own.")
}

/// The Language's options: those its `@Flag` and `@Option` members declare,
/// with their help, and those `flags` declares.
fn language_options(metadata: &FrontendMetadata) -> Vec<Arg> {
    metadata
        .flags
        .iter()
        .map(|flag| {
            if let Some(declared) = metadata.cli.args.iter().find(|arg| arg.id == flag.name) {
                return declared.clone();
            }
            match &flag.default_value {
                DriverFlagValue::Boolean(default) => {
                    let mut arg = Arg::new(&flag.name, ArgKind::Flag, ValueType::Bool);
                    arg.default = default.then(|| "true".to_owned());
                    arg
                }
                DriverFlagValue::Integer(default) => {
                    Arg::new(&flag.name, ArgKind::Option, ValueType::Int)
                        .default_text(&default.to_string())
                }
                DriverFlagValue::String(default) => {
                    Arg::new(&flag.name, ArgKind::Option, ValueType::String).default_text(default)
                }
            }
        })
        .collect()
}

fn compile(name: &str, about: &str, metadata: &FrontendMetadata, emit: bool) -> Command {
    let mut command = Command::new(name)
        .about(about)
        .builtin(name)
        .arg(if name == "build" {
            inputs("Source files, or directories holding them.").required()
        } else {
            inputs("Source files, or directories holding them. Defaults to the current package.")
        })
        .arg(
            Arg::new("output", ArgKind::Option, ValueType::String)
                .short('o')
                .value_name("path")
                .help("Where to write the output."),
        );
    if emit {
        command = command.arg(
            Arg::new(
                "emit",
                ArgKind::Option,
                ValueType::Choice(EMIT_KINDS.iter().map(|kind| (*kind).to_owned()).collect()),
            )
            .help("What to write. Without it, -o's extension decides: .kir, .klbc, .a, .ridl, .so or .dylib, else an executable; with neither, KIR."),
        );
    }
    command = command
        .arg(
            Arg::new("export", ArgKind::Option, ValueType::String)
                .value_name("function")
                .help("Keep this function exported when writing a library."),
        )
        .arg(target())
        .arg(timings());
    if name == "build" {
        command = command.arg(
            Arg::new("depfile", ArgKind::Option, ValueType::String)
                .value_name("path")
                .help("Write every file the compile read, as a make-style depfile."),
        );
        command = command.arg(
            Arg::new("ridl-source", ArgKind::Flag, ValueType::Bool).help(
                "Mark each declaration of an emitted .ridl with @Source, the place it is written.",
            ),
        );
        command = command.arg(
            Arg::new("contract", ArgKind::Option, ValueType::String)
                .value_name("path")
                .help("With --emit shared, also write the library's .ridl contract here."),
        );
    }
    command.args.extend(language_options(metadata));
    command.arg(
        Arg::new("program", ArgKind::Rest, ValueType::String)
            .many()
            .value_name("arguments")
            .help("Arguments the program runs with."),
    )
}

/// The driver's whole command tree.
#[must_use]
pub fn command_tree(metadata: &FrontendMetadata) -> Command {
    let mut root = Command::new(&metadata.driver);
    root.version = Some(metadata.version.clone());
    root.abstract_text = if metadata.cli.abstract_text.is_empty() {
        format!("The {} compiler.", metadata.name)
    } else {
        metadata.cli.abstract_text.clone()
    };
    let mut discussion = metadata.cli.discussion.clone();
    if !metadata.extensions.is_empty() {
        let extensions: Vec<String> = metadata
            .extensions
            .iter()
            .map(|e| format!(".{e}"))
            .collect();
        if !discussion.is_empty() {
            discussion.push_str("\n\n");
        }
        let _ = write!(discussion, "Sources: {}", extensions.join(" "));
    }
    for mode in &metadata.file_modes {
        let _ = write!(
            discussion,
            "\n{}=<glob> compiles the matching files in the `{}` mode.",
            mode.flag, mode.name
        );
    }
    root.discussion = discussion;
    root.default_subcommand = Some("build".to_owned());
    root.subcommands
        .push(compile("build", "Compile sources.", metadata, true));
    root.subcommands.push(compile(
        "run",
        "Build an executable and run it.",
        metadata,
        false,
    ));
    if metadata.declares(crate::Hook::CompatDriver)
        && metadata.flags.iter().any(|flag| flag.name == "test")
    {
        root.subcommands.push(compile(
            "test",
            "Build the package's tests and run them.",
            metadata,
            false,
        ));
    }
    let mut check = Command::new("check")
        .about("Report diagnostics without writing anything.")
        .builtin("check")
        .arg(inputs(
            "Source files, or directories holding them. Without any, the package in the current directory.",
        ))
        .arg(target())
        .arg(timings());
    check.args.extend(language_options(metadata));
    root.subcommands.push(check);
    if metadata.declares(crate::Hook::Format) {
        let mut format = Command::new("format")
            .about("Format sources in place.")
            .builtin("format")
            .arg(inputs("Source files, or directories holding them."))
            .arg(Arg::new("check", ArgKind::Flag, ValueType::Bool).help("List the files that are not formatted instead of formatting them."))
            .arg(Arg::new("stdin", ArgKind::Option, ValueType::String).value_name("name").help("Format standard input as the file `name` and write the result to standard output."))
            .arg(Arg::new("width", ArgKind::Option, ValueType::Int).default_text("100").help("The line width."))
            .arg(Arg::new("tabSize", ArgKind::Option, ValueType::Int).default_text("4").help("The indentation width."))
            .arg(Arg::new("tabs", ArgKind::Flag, ValueType::Bool).help("Indent with tabs."))
            .arg(target());
        format.args.extend(language_options(metadata));
        root.subcommands.push(format);
    }
    // Section 5.5: `<driver> push <host:port>`, the hot module push client. It
    // compiles the package in the current directory to KIR, which the device
    // lowers (section 17.5), so it takes what `check` takes plus the device.
    let mut push = Command::new("push")
        .about("Compile the package in the current directory and push it to a running device.")
        .builtin("push")
        .arg(
            Arg::new("device", ArgKind::Positional, ValueType::String)
                .required()
                .value_name("host:port")
                .help("The device to push to: any address it listens on, a tailnet one included."),
        )
        .arg(target());
    push.args.extend(language_options(metadata));
    root.subcommands.push(push);
    root.subcommands.push(
        Command::new("lsp")
            .about("Serve the language server on standard input and output.")
            .builtin("lsp"),
    );
    let mut worker = Command::new("lsp-worker").builtin("lsp-worker");
    worker.hidden = true;
    root.subcommands.push(worker);
    root.subcommands
        .extend(metadata.cli.subcommands.iter().cloned());
    root
}

/// Checks the tree and the Language's flags, as `klf build` does before it
/// writes a driver.
///
/// # Errors
/// Returns what is wrong: a flag the session refuses, or a name the tree
/// declares twice or reserves.
pub fn validate(metadata: &FrontendMetadata) -> Result<(), String> {
    Session::validate_driver_flags(&metadata.flags)?;
    command_tree(metadata).validate()
}

/// A command line with its file mode selections taken out.
pub struct FileModeSplit {
    /// The arguments that select no file mode.
    pub rest: Vec<String>,
    /// Each selection's flag and the glob it selects.
    pub globs: Vec<(String, String)>,
}

/// Takes `<flag>=<glob>` file mode selections out of `arguments`.
///
/// # Errors
/// Returns a message for a selection without a glob.
pub fn split_file_modes(arguments: &[String], modes: &[FileMode]) -> Result<FileModeSplit, String> {
    let mut rest = Vec::new();
    let mut globs = Vec::new();
    for argument in arguments {
        if let Some((flag, glob)) = argument.split_once('=')
            && modes.iter().any(|mode| mode.flag == flag)
        {
            if glob.is_empty() {
                return Err(format!("`{flag}` needs a file glob"));
            }
            globs.push((flag.to_owned(), glob.to_owned()));
        } else {
            rest.push(argument.clone());
        }
    }
    Ok(FileModeSplit { rest, globs })
}

/// What the `build`, `run`, `check` or `format` command `matches` selected
/// asks for.
#[must_use]
pub fn invocation(
    root: &Command,
    matches: &Matches,
    metadata: &FrontendMetadata,
    globs: Vec<(String, String)>,
) -> Invocation {
    let command = matches.command(root);
    let output = matches.string(root, "output").map(PathBuf::from);
    let emit = match &command.action {
        Action::Builtin(key) if key == "run" => "run".to_owned(),
        Action::Builtin(key) if key == "check" => "check".to_owned(),
        // A push compiles to KIR and sends it rather than writing it.
        Action::Builtin(key) if key == "push" => "kir".to_owned(),
        _ => matches.string(root, "emit").unwrap_or_else(|| {
            match output
                .as_ref()
                .and_then(|output| output.extension())
                .and_then(|extension| extension.to_str())
            {
                None if output.is_none() => "kir",
                Some("kir") => "kir",
                Some("klbc") => "klbc",
                Some("a") => "lib",
                Some("ridl") => "ridl",
                Some("dylib" | "so") => "shared",
                _ => "bin",
            }
            .to_owned()
        }),
    };
    let mut flag_overrides = HashMap::new();
    for flag in &metadata.flags {
        if !matches.present(root, &flag.name) {
            continue;
        }
        let value = match &flag.default_value {
            DriverFlagValue::Boolean(_) => {
                matches.bool(root, &flag.name).map(DriverFlagValue::Boolean)
            }
            DriverFlagValue::Integer(_) => {
                matches.int(root, &flag.name).map(DriverFlagValue::Integer)
            }
            // A repeatable option's values, one per line.
            DriverFlagValue::String(_)
                if metadata
                    .cli
                    .args
                    .iter()
                    .any(|arg| arg.id == flag.name && arg.arity == Arity::Many) =>
            {
                Some(DriverFlagValue::String(
                    matches.strings(root, &flag.name).join("\n"),
                ))
            }
            DriverFlagValue::String(_) => matches
                .string(root, &flag.name)
                .map(DriverFlagValue::String),
        };
        if let Some(value) = value {
            flag_overrides.insert(flag.name.clone(), value);
        }
    }
    // `build`'s inputs are required and `run` and `test` name their own, so
    // an invocation with none is `check` with none, and it means the package in
    // the current directory as its own help says.
    let mut inputs: Vec<PathBuf> = matches
        .strings(root, "inputs")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if inputs.is_empty() {
        inputs.push(PathBuf::from("."));
    }
    Invocation {
        inputs,
        output,
        export: matches.string(root, "export"),
        emit,
        target: matches.string(root, "target"),
        flag_overrides,
        file_mode_globs: globs,
        program_arguments: matches.strings(root, "program"),
        device: matches.string(root, "device"),
        timings: matches.bool(root, "timings").unwrap_or(false),
        depfile: matches.string(root, "depfile").map(PathBuf::from),
        ridl_source: matches.bool(root, "ridl-source").unwrap_or(false),
        contract: matches.string(root, "contract").map(PathBuf::from),
    }
}

/// Reads `arguments` as `build` does: the driver arguments a language server
/// workspace gives each analysis.
///
/// # Errors
/// Returns the rendered command-line error.
pub fn parse_build_arguments(
    arguments: &[String],
    metadata: &FrontendMetadata,
) -> Result<Invocation, String> {
    let mut build = compile("build", "", metadata, true);
    build.name.clone_from(&metadata.driver);
    for arg in &mut build.args {
        if arg.id == "inputs" {
            arg.required = false;
            arg.arity = Arity::Many;
        }
    }
    let FileModeSplit { rest, globs } = split_file_modes(arguments, &metadata.file_modes)?;
    match klf_args::parse(&build, &rest) {
        Ok(klf_args::Outcome::Run(matches)) => Ok(invocation(&build, &matches, metadata, globs)),
        Ok(_) => Err("the driver arguments ask for help, not a build".to_owned()),
        Err(error) => Err(error.render(&build)),
    }
}
