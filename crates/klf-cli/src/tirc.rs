use std::path::Path;
use std::process::ExitCode;

use tir_diag::{Diagnostic, Severity};

pub(crate) fn command(args: &[String]) -> ExitCode {
    match args {
        [operation, files @ ..] if operation == "check" && !files.is_empty() => check(files),
        [operation, input, option, output] if operation == "asm" && option == "-o" => {
            assemble(input, output)
        }
        [operation, input] if operation == "dis" => disassemble(input),
        [operation, files @ ..] if operation == "canonicalize" && !files.is_empty() => {
            canonicalize(files)
        }
        [help] if help == "--help" || help == "-h" => {
            println!(
                "usage: tirc check <files> | asm <file.tir> -o <file.tirb> | dis <file.tirb> | canonicalize <files>"
            );
            ExitCode::SUCCESS
        }
        [] => fail(
            "usage: tirc check <files> | asm <file.tir> -o <file.tirb> | dis <file.tirb> | canonicalize <files>",
        ),
        [operation, ..] if operation == "check" => fail("tirc check: expected at least one file"),
        [operation, ..] if operation == "asm" => {
            fail("tirc asm: expected <file.tir> -o <file.tirb>")
        }
        [operation, ..] if operation == "dis" => fail("tirc dis: expected <file.tirb>"),
        [operation, ..] if operation == "canonicalize" => {
            fail("tirc canonicalize: expected one or more textual .tir files")
        }
        [operation, ..] => fail(&format!("tirc: unknown command '{operation}'")),
    }
}

fn canonicalize(files: &[String]) -> ExitCode {
    // Collect all outputs before writing any bytes to stdout. A rejected
    // ownership pattern must never produce a partial canonicalized stream.
    let mut outputs = Vec::with_capacity(files.len());
    for path in files {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                return fail(&format!("tirc canonicalize: cannot read '{path}': {error}"));
            }
        };
        match tir_own::canonicalize_text(&source) {
            Ok(canonical) => outputs.push(canonical),
            Err(error) => return fail(&format!("tirc canonicalize: {path}: {error}")),
        }
    }
    for canonical in outputs {
        print!("{canonical}");
    }
    ExitCode::SUCCESS
}

fn assemble(input: &str, output: &str) -> ExitCode {
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("tirc: cannot read '{input}': {error}");
            return ExitCode::FAILURE;
        }
    };
    let source = match std::str::from_utf8(&bytes) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{input}: TIR text is not UTF-8: {error}");
            return ExitCode::FAILURE;
        }
    };
    let module = match tir_format::parse_text_module(source) {
        Ok(module) => module,
        Err(error) => {
            let (line, column) = line_column(source, error.span.start);
            eprintln!("{input}:{line}:{column}: error: {}", error.message);
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for diagnostic in tir_verify::verify_text_module(&module, Default::default()) {
        failed |= diagnostic.severity == Severity::Error;
        render_diagnostic(input, source, &diagnostic);
    }
    if failed {
        return ExitCode::FAILURE;
    }
    let binary = match tir_format::assemble_text_module(&module) {
        Ok(binary) => binary,
        Err(error) => {
            let (line, column) = line_column(source, error.span.start);
            eprintln!("{input}:{line}:{column}: error: {}", error.message);
            return ExitCode::FAILURE;
        }
    };
    let decoded = match tir_format::BinaryModule::decode(&binary) {
        Ok(module) => module,
        Err(error) => return fail(&format!("{input}: assembled binary is invalid: {error}")),
    };
    let mut binary_failed = false;
    for diagnostic in tir_verify::verify_binary_module_records(&decoded, u64::MAX) {
        binary_failed |= diagnostic.severity == Severity::Error;
        render_binary_diagnostic(output, &diagnostic);
    }
    if binary_failed {
        return ExitCode::FAILURE;
    }
    match std::fs::write(output, binary) {
        Ok(()) => {
            println!("Assembled: Ok {output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tirc: cannot write '{output}': {error}");
            ExitCode::FAILURE
        }
    }
}

fn disassemble(input: &str) -> ExitCode {
    let bytes = match std::fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("tirc: cannot read '{input}': {error}");
            return ExitCode::FAILURE;
        }
    };
    let module = match tir_format::BinaryModule::decode(&bytes) {
        Ok(module) => module,
        Err(error) => {
            eprintln!("{input}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for diagnostic in tir_verify::verify_binary_module_records(&module, u64::MAX) {
        failed |= diagnostic.severity == Severity::Error;
        render_binary_diagnostic(input, &diagnostic);
    }
    if failed {
        return ExitCode::FAILURE;
    }
    match tir_format::disassemble_text_module(&module, u64::MAX) {
        Ok(text) => {
            let round_trip = tir_format::parse_text_module(&text)
                .map_err(|error| error.to_string())
                .and_then(|parsed| {
                    tir_format::assemble_text_module(&parsed).map_err(|error| error.to_string())
                });
            match round_trip {
                Ok(reassembled) if reassembled == bytes => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Ok(_) => fail(&format!(
                    "{input}: TIR-ENC-0002: canonical disassembly does not reproduce the original bytes"
                )),
                Err(error) => fail(&format!(
                    "{input}: disassembled text cannot be reassembled: {error}"
                )),
            }
        }
        Err(error) => {
            eprintln!("{input}: {error}");
            ExitCode::FAILURE
        }
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
                    let mut records_failed = false;
                    for diagnostic in tir_verify::verify_binary_module_records(&module, u64::MAX) {
                        records_failed |= diagnostic.severity == Severity::Error;
                        render_binary_diagnostic(path, &diagnostic);
                    }
                    if records_failed {
                        failed = true;
                    } else {
                        println!("Check: Ok {path}");
                    }
                }
                Err(error) => {
                    eprintln!("{path}: {error}");
                    failed = true;
                }
            }
            continue;
        }
        if bytes.starts_with(b"TVIS") {
            let image =
                match tvisa_format::TvisaImage::decode_and_validate_sections(&bytes, u64::MAX) {
                    Ok(image) => image,
                    Err(error) => {
                        eprintln!("{path}: {error}");
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
