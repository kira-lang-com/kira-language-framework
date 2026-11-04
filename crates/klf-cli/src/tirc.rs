use std::path::Path;
use std::process::ExitCode;

use tir_diag::{Diagnostic, Severity};

mod analyze;
mod asm;
mod dis;
mod jcs;
mod package;

pub(crate) fn command(args: &[String]) -> ExitCode {
    match args {
        [operation, files @ ..] if operation == "check" && !files.is_empty() => check(files),
        [operation, input, option, output] if operation == "asm" && option == "-o" => {
            asm::command(input, output)
        }
        [operation, path] if operation == "dis" => dis::command(path),
        [operation, path] if operation == "analyze" => analyze::command(path),
        [operation, arguments @ ..] if operation == "package" => package::command(arguments),
        [help] if help == "--help" || help == "-h" => {
            println!(
                "usage: tirc check <files>\n       tirc asm <file.tir> -o <file.tirb>\n       tirc dis <file.tirb>\n       tirc analyze <pkg>\n       tirc package <dir> --manifest <file> --sign <identity> -o <pkg.tirpkg>"
            );
            ExitCode::SUCCESS
        }
        [] => fail("usage: tirc check <files>"),
        [operation, ..] if operation == "check" => fail("tirc check: expected at least one file"),
        [operation, ..] if operation == "asm" => fail("usage: tirc asm <file.tir> -o <file.tirb>"),
        [operation, ..] if operation == "dis" => fail("usage: tirc dis <file.tirb>"),
        [operation, ..] if operation == "analyze" => fail("usage: tirc analyze <pkg>"),
        [operation, ..] => fail(&format!("tirc: unknown command '{operation}'")),
    }
}

fn check(files: &[String]) -> ExitCode {
    let mut failed = false;
    for path in files {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("tirc: cannot read '{path}': {error}");
                failed = true;
                continue;
            }
        };
        if bytes.starts_with(b"TIRB")
            || Path::new(path)
                .extension()
                .is_some_and(|extension| extension == "tirb")
        {
            match tir_format::BinaryModule::decode(&bytes) {
                Ok(module) => {
                    let mut file_failed = false;
                    for diagnostic in tir_verify::verify_binary_module_records(&module, u64::MAX) {
                        file_failed |= diagnostic.severity == Severity::Error;
                        render_binary_diagnostic(path, &diagnostic);
                    }
                    if file_failed {
                        failed = true;
                    } else {
                        println!("Check: Ok {path}");
                    }
                }
                Err(error) => {
                    render_binary_diagnostic(path, &error.diagnostic);
                    failed = true;
                }
            }
            continue;
        }
        if bytes.starts_with(b"TVIS")
            || Path::new(path)
                .extension()
                .is_some_and(|extension| extension == "tvisa")
        {
            let image =
                match tvisa_format::TvisaImage::decode_and_validate_sections(&bytes, u64::MAX) {
                    Ok(image) => image,
                    Err(error) => {
                        render_binary_diagnostic(path, &error.diagnostic);
                        failed = true;
                        continue;
                    }
                };
            let mut file_failed = false;
            for diagnostic in tvisa_verify::verify_binary_image(&image, u64::MAX) {
                file_failed |= diagnostic.severity == Severity::Error;
                render_binary_diagnostic(path, &diagnostic);
            }
            if file_failed {
                failed = true;
            } else {
                println!("Check: Ok {path}");
            }
            continue;
        }
        let source = match std::str::from_utf8(&bytes) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{path}: TIR text is not UTF-8: {error}");
                failed = true;
                continue;
            }
        };
        if first_content_line(source).is_some_and(|line| line.starts_with("tvisa ")) {
            let image = match tvisa_format::parse_text_image(source) {
                Ok(image) => image,
                Err(error) => {
                    let (line, column) = line_column(source, error.span.start);
                    eprintln!("{path}:{line}:{column}: error: {}", error.message);
                    failed = true;
                    continue;
                }
            };
            let diagnostics = tvisa_verify::verify_text_image(&image);
            let mut file_failed = false;
            for diagnostic in diagnostics {
                file_failed |= diagnostic.severity == Severity::Error;
                render_diagnostic(path, source, &diagnostic);
            }
            if file_failed {
                failed = true;
            } else {
                println!("Check: Ok {path}");
            }
            continue;
        }
        let module = match tir_format::parse_text_module(source) {
            Ok(module) => module,
            Err(error) => {
                let (line, column) = line_column(source, error.span.start);
                eprintln!("{path}:{line}:{column}: error: {}", error.message);
                failed = true;
                continue;
            }
        };
        let diagnostics = tir_verify::verify_text_module(&module, Default::default());
        let mut file_failed = false;
        for diagnostic in diagnostics {
            file_failed |= diagnostic.severity == Severity::Error;
            render_diagnostic(path, source, &diagnostic);
        }
        if file_failed {
            failed = true;
        } else {
            println!("Check: Ok {path}");
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn first_content_line(source: &str) -> Option<&str> {
    source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with(';'))
}

fn render_diagnostic(path: &str, source: &str, diagnostic: &Diagnostic) {
    if let Some(span) = diagnostic.primary_span {
        let (line, column) = line_column(source, span.start);
        eprintln!("{path}:{line}:{column}: {diagnostic}");
    } else {
        eprintln!("{path}: {diagnostic}");
    }
    for label in &diagnostic.labels {
        let (line, column) = line_column(source, label.span.start);
        eprintln!("  {path}:{line}:{column}: {}", label.message);
    }
    for note in &diagnostic.notes {
        if let Some(span) = note.span {
            let (line, column) = line_column(source, span.start);
            eprintln!("  {path}:{line}:{column}: note: {}", note.message);
        } else {
            eprintln!("  note: {}", note.message);
        }
    }
}

fn render_binary_diagnostic(path: &str, diagnostic: &Diagnostic) {
    if let Some(span) = diagnostic.primary_span {
        eprintln!("{path}:0x{:X}: {diagnostic}", span.start);
    } else {
        eprintln!("{path}: {diagnostic}");
    }
    for label in &diagnostic.labels {
        eprintln!("  {path}:0x{:X}: {}", label.span.start, label.message);
    }
    for note in &diagnostic.notes {
        if let Some(span) = note.span {
            eprintln!("  {path}:0x{:X}: note: {}", span.start, note.message);
        } else {
            eprintln!("  note: {}", note.message);
        }
    }
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &source[..end];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    (line, column)
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
