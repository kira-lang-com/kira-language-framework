use std::process::ExitCode;

/// `klf ksir verify|dump <file.ksir>`: parses a text module, then validates
/// it (`KLF-KSL.md` section 16) or prints it in normal form.
pub fn command(args: &[String]) -> ExitCode {
    let (Some(operation), Some(path), None) = (args.first(), args.get(1), args.get(2)) else {
        return fail("usage: klf ksir verify|dump <file.ksir>");
    };
    if operation != "verify" && operation != "dump" {
        return fail(&format!("klf ksir: unknown subcommand `{operation}`"));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return fail(&format!("klf ksir: cannot read `{path}`: {error}")),
    };
    let module = match klf_shader::parse::parse(&text) {
        Ok(module) => module,
        Err(error) => return fail(&format!("{path}: {error}")),
    };
    if operation == "dump" {
        klf_diag::raw!("{}", klf_shader::print::print(&module));
        return ExitCode::SUCCESS;
    }
    let validation = klf_shader_valid::validate(&module, klf_shader_valid::Options::default());
    let mut failed = false;
    for problem in &validation.problems {
        let severity = if problem.warning { "warning" } else { "error" };
        failed |= !problem.warning;
        klf_diag::err!("{path}: {severity}: {problem}");
    }
    if failed {
        return ExitCode::FAILURE;
    }
    klf_diag::out!("Check: Ok {path}");
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("{message}");
    ExitCode::FAILURE
}
