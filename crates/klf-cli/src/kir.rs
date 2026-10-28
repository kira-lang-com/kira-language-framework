use klf_kir::parse_module;
use klf_kir_verify::verify_module;
use std::process::ExitCode;

pub fn command(args: &[String]) -> ExitCode {
    let Some(operation) = args.first().map(String::as_str) else {
        return fail("usage: klf kir verify|dump <file.kir>");
    };
    let Some(path) = args.get(1) else {
        return fail(&format!("klf kir {operation}: expected a .kir file"));
    };
    if args.len() != 2 {
        return fail(&format!(
            "klf kir {operation}: unexpected argument `{}`",
            args[2]
        ));
    }
    if operation != "verify" && operation != "dump" {
        return fail(&format!("klf kir: unknown subcommand `{operation}`"));
    }

    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return fail(&format!("klf kir: cannot read `{path}`: {error}")),
    };
    let module = match parse_module(&text) {
        Ok(module) => module,
        Err(error) => return fail(&format!("{path}: {error:?}")),
    };

    match operation {
        "dump" => klf_diag::raw!("{module}"),
        "verify" => {
            let diagnostics = verify_module(&module);
            if !diagnostics.is_empty() {
                for diagnostic in diagnostics {
                    klf_diag::err!("{path}: {diagnostic}");
                }
                return ExitCode::FAILURE;
            }
            klf_diag::out!("Check: Ok {path}");
        }
        _ => unreachable!(),
    }
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("{message}");
    ExitCode::FAILURE
}
