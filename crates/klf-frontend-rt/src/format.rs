//! `<driver> format [paths] [--check] [--stdin NAME] [--width N]
//! [--tab-size N] [--tabs]` (`KLF-FORMAT.md` section 6), parsed with the
//! driver's command tree.
//!
//! Each file is formatted in a session of its own. `--stdin NAME` formats
//! standard input as the file `NAME` and writes the result to standard
//! output; the language server formats through it. Other driver flags are
//! taken as for a build, so file modes select as they would there.

use crate::{FrontendMetadata, NativePhases, add_inputs, expand_inputs, klffe_phase, new_session};
use klf_args::{Command, Matches};
use klf_defsvc::driver_cli::invocation;
use klf_defsvc::{FormatRunner, close_format, format_file, open_format};
use klf_format::{Builder, Options, Syntax};
use klf_native_rt::services;
use klf_services::{DefinitionPhaseArguments, Session};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

const FORMAT_SELECTOR: i64 = 7;

impl FormatRunner for NativePhases {
    fn format(
        &mut self,
        session: &mut Session,
        arguments: &DefinitionPhaseArguments,
        syntax: Syntax,
    ) -> Result<(Builder, bool), String> {
        let (bridge, words, source) = open_format(session, arguments, syntax)?;
        let mut sink = klf_defsvc::ServiceSink::new();
        bridge.register(&mut sink);
        services::install(sink.into_table(), source);
        let argument = |index: usize| words.get(index).map_or(0, |word| *word as i64);
        // SAFETY: `klffe_phase` is emitted into this binary with exactly this
        // signature; selector 7 calls the generated `format` entry with the
        // session, unit and tree words `open_format` marshalled.
        let word = unsafe { klffe_phase(FORMAT_SELECTOR, argument(0), argument(1), argument(2)) };
        let reported = services::uninstall();
        close_format(&bridge, session, &reported, word as u64)
    }
}

struct Request {
    check: bool,
    stdin: Option<PathBuf>,
    options: Options,
}

fn request(root: &Command, matches: &Matches) -> Result<Request, String> {
    let mut options = Options::default();
    if let Ok(executable) = std::env::current_exe() {
        let directory = executable
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let root_directory = if directory.file_name().is_some_and(|name| name == "bin") {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        let path = root_directory.join("formatter.json");
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let config: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| format!("invalid {}: {error}", path.display()))?;
            if let Some(width) = config["width"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
            {
                options.width = width;
            }
            if let Some(size) = config["tabSize"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
            {
                options.tab_size = size;
            }
            if let Some(spaces) = config["insertSpaces"].as_bool() {
                options.insert_spaces = spaces;
            }
        }
    }
    let positive = |name: &str, value: Option<i64>, current: usize| match value {
        None => Ok(current),
        Some(value) => usize::try_from(value)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("--{name} takes a positive number, not `{value}`")),
    };
    if matches.present(root, "width") {
        options.width = positive("width", matches.int(root, "width"), options.width)?;
    }
    if matches.present(root, "tabSize") {
        options.tab_size = positive("tab-size", matches.int(root, "tabSize"), options.tab_size)?;
    }
    if matches.bool(root, "tabs") == Some(true) {
        options.insert_spaces = false;
    }
    Ok(Request {
        check: matches.bool(root, "check") == Some(true),
        stdin: matches.string(root, "stdin").map(PathBuf::from),
        options,
    })
}

/// Runs the command and answers the exit status.
pub(crate) fn run(
    metadata: &FrontendMetadata,
    root: &Command,
    matches: &Matches,
    globs: Vec<(String, String)>,
) -> i32 {
    match command(metadata, root, matches, globs) {
        Ok(status) => status,
        Err(message) => {
            klf_diag::err!("{}: error: {message}", metadata.driver);
            1
        }
    }
}

fn command(
    metadata: &FrontendMetadata,
    root: &Command,
    matches: &Matches,
    globs: Vec<(String, String)>,
) -> Result<i32, String> {
    if !metadata.declares(klf_defsvc::Hook::Format) {
        return Err(format!("{} has no formatter", metadata.name));
    }
    let request = request(root, matches)?;
    let mut invocation = invocation(root, matches, metadata, globs);
    if request.stdin.is_none() && invocation.inputs.is_empty() {
        return Err("format needs files, directories or --stdin NAME".to_owned());
    }
    let triple = invocation
        .target
        .clone()
        .unwrap_or_else(|| metadata.target.clone());
    let syntax = Syntax::from_spec(&metadata.editor_spec());
    if let Some(name) = &request.stdin {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("cannot read standard input: {error}"))?;
        invocation.inputs = vec![name.clone()];
        let mut session = new_session(metadata, &invocation, &triple)?;
        session.set_overlay([(name.clone(), Arc::from(text.as_str()))]);
        let formatted = format_one(
            &mut session,
            metadata,
            &invocation,
            &syntax,
            &request.options,
        )?;
        std::io::stdout()
            .write_all(formatted.as_bytes())
            .map_err(|error| format!("cannot write standard output: {error}"))?;
        return Ok(0);
    }
    let mut status = 0;
    for path in expand_inputs(&invocation.inputs.clone(), &metadata.extensions)? {
        invocation.inputs = vec![path.clone()];
        let mut session = new_session(metadata, &invocation, &triple)?;
        let original = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let formatted = match format_one(
            &mut session,
            metadata,
            &invocation,
            &syntax,
            &request.options,
        ) {
            Ok(formatted) => formatted,
            Err(message) => {
                klf_diag::err!("{}: error: {}: {message}", metadata.driver, path.display());
                status = 1;
                continue;
            }
        };
        if formatted == original {
            continue;
        }
        if request.check {
            klf_diag::out!("{}", path.display());
            status = 1;
        } else {
            std::fs::write(&path, formatted)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
    }
    Ok(status)
}

fn format_one(
    session: &mut Session,
    metadata: &FrontendMetadata,
    invocation: &crate::arguments::Invocation,
    syntax: &Syntax,
    options: &Options,
) -> Result<String, String> {
    let files = add_inputs(session, metadata, invocation)?;
    let file = *files.first().ok_or("no file to format")?;
    let mut make_runner = || Ok(NativePhases::new());
    format_file(session, file, syntax, options, &mut make_runner).map_err(|error| error.to_string())
}
