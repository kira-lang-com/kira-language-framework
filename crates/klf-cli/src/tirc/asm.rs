use std::fs;
use std::process::ExitCode;

use tir_diag::Severity;

pub(super) fn command(input: &str, output: &str) -> ExitCode {
    let source = match fs::read_to_string(input) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("tirc asm: cannot read '{input}': {error}");
            return ExitCode::FAILURE;
        }
    };
    let bytes = match assemble(&source) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("tirc asm: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(output, bytes) {
        eprintln!("tirc asm: cannot write '{output}': {error}");
        return ExitCode::FAILURE;
    }
    println!("Assembled: Ok {output}");
    ExitCode::SUCCESS
}

pub(super) fn assemble(source: &str) -> Result<Vec<u8>, String> {
    let module = tir_format::parse_text_module(source).map_err(|error| error.to_string())?;
    let errors = tir_verify::verify_text_module(&module, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    tir_format::assemble_text_module(&module).map_err(|error| error.to_string())
}
