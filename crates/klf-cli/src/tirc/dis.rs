use std::fs;
use std::process::ExitCode;

use tir_diag::Severity;
use tir_format::BinaryModule;

pub(super) fn command(input: &str) -> ExitCode {
    let bytes = match fs::read(input) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("tirc dis: cannot read '{input}': {error}");
            return ExitCode::FAILURE;
        }
    };
    match disassemble(&bytes) {
        Ok(source) => {
            print!("{source}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("tirc dis: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(super) fn disassemble(bytes: &[u8]) -> Result<String, String> {
    let module = BinaryModule::decode(bytes).map_err(|error| error.to_string())?;
    let errors = tir_verify::verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let source =
        tir_format::disassemble_binary_module(&module).map_err(|error| error.to_string())?;
    if super::asm::assemble(&source)? != bytes {
        return Err("canonical disassembly does not re-encode to the input bytes".to_owned());
    }
    Ok(source)
}
