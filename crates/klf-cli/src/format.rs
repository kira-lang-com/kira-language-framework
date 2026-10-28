//! `klf format <definition-package-dir> [files] [--check] [--width N]`: a
//! definition's `format` hook on the VM, for working on a formatter without
//! building its toolchain (`KLF-FORMAT.md` section 6). Without files it
//! formats standard input to standard output.

use klf_driver::{Backend, LanguageDefinition};
use klf_format::Options;
use klf_services::Session;
use klf_target::Target;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

pub fn command(args: &[String]) -> ExitCode {
    let mut options = Options::default();
    let mut check = false;
    let mut timings = false;
    let mut paths = Vec::new();
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--check" => check = true,
            "--timings" => timings = true,
            "--width" => match args.next().and_then(|width| width.parse().ok()) {
                Some(width) => options.width = width,
                None => return fail("--width takes a number"),
            },
            _ => paths.push(argument.clone()),
        }
    }
    let Some((root, files)) = paths.split_first() else {
        return fail(
            "usage: klf format <definition-package-dir> [files] [--check] [--width N] [--timings]",
        );
    };
    let mut finish = klf_diag::status::begin(&format!("Formatting {root}"), timings);
    let target = match crate::resolve_target(crate::default_target()) {
        Ok(target) => target,
        Err(message) => return fail(&message),
    };
    let mut session = Session::new(target.clone());
    let Some(definition) = LanguageDefinition::load(&mut session, Path::new(root), Backend::Vm)
    else {
        klf_diag::raw_err!("{}", crate::pipeline::render(&session));
        return fail("klf format: definition package did not compile");
    };
    let std_root = definition.descriptor().std_root.clone();
    let std_root = (!std_root.is_empty()).then(|| Path::new(root).join(std_root));
    let session_for = |target: &Target| {
        let mut session = Session::new(target.clone());
        if let Some(std_root) = &std_root {
            session.set_std_root(std_root.to_string_lossy().into_owned());
        }
        session
    };
    if files.is_empty() {
        let mut text = String::new();
        if std::io::stdin().read_to_string(&mut text).is_err() {
            return fail("klf format: cannot read standard input");
        }
        let name = format!(
            "input.{}",
            definition
                .descriptor()
                .extensions
                .first()
                .map_or("", String::as_str)
        );
        let formatted =
            match definition.format_source(&mut session_for(&target), &name, &text, &options) {
                Ok(formatted) => formatted,
                Err(error) => {
                    finish.end();
                    return fail(&format!("klf format: {error}"));
                }
            };
        finish.end();
        klf_diag::raw!("{formatted}");
        return ExitCode::SUCCESS;
    }
    // Each file is a phase: a formatter over a package reports which file it is
    // on, and a `--check` run over fifty files says which one is wrong.
    let mut needs_formatting = Vec::new();
    let mut status = ExitCode::SUCCESS;
    for file in files {
        klf_diag::progress::report(&format!("format {file}"));
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                finish.end();
                return fail(&format!("klf format: cannot read {file}: {error}"));
            }
        };
        match definition.format_source(&mut session_for(&target), file, &text, &options) {
            Ok(formatted) if formatted == text => {}
            Ok(_) if check => needs_formatting.push(file.to_owned()),
            Ok(formatted) => {
                if let Err(error) = std::fs::write(file, formatted) {
                    finish.end();
                    return fail(&format!("klf format: cannot write {file}: {error}"));
                }
            }
            Err(error) => {
                klf_diag::err!("klf format: {file}: {error}");
                status = ExitCode::FAILURE;
            }
        }
    }
    finish.end();
    for file in &needs_formatting {
        klf_diag::out!("{file}");
        status = ExitCode::FAILURE;
    }
    status
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("{message}");
    ExitCode::FAILURE
}
