//! `klf push <host:port>`: the hot module push client of section 5.5, driving
//! the section 17.5 exchange.
//!
//! The client compiles to KIR and the device lowers it, so what crosses the
//! network is the portable form. A device behind a tailnet is addressed by its
//! tailnet address like any other.

use crate::pipeline;
use klf_diag::{err, out};
use klf_live::client::{Client, Pushed};
use std::process::ExitCode;

/// `klf push <file> <host:port> [--entry NAME]`, which compiles `file` to KIR
/// and pushes it to a running device.
pub fn command(arguments: &[String]) -> ExitCode {
    let Some(file) = arguments.first() else {
        return fail("usage: klf push <file> <host:port>");
    };
    let Some(address) = arguments.get(1) else {
        return fail("klf push needs a device address, as <host:port>");
    };
    let entry = entry_of(&arguments[2..]);
    let mut finish = klf_diag::status::begin(&format!("Pushing {file} to {address}"), false);

    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            finish.end();
            return fail(&format!("cannot read `{file}`: {error}"));
        }
    };
    let name = std::path::Path::new(file)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Main")
        .to_owned();

    let module = match compile(&source, file, &name) {
        Ok(module) => module,
        Err(rendered) => {
            finish.end();
            err!("{rendered}");
            out!("Pushed: Failed");
            return ExitCode::FAILURE;
        }
    };
    let module_name = module.name.as_str().to_owned();

    let mut client = match Client::connect(address) {
        Ok(client) => client,
        Err(message) => {
            finish.end();
            return fail(&message);
        }
    };
    let hello = match client.handshake(&module_name) {
        Ok(hello) => hello,
        Err(message) => {
            finish.end();
            return fail(&message);
        }
    };
    finish.end();
    out!(
        "Hello: Ok {} ({} module layouts)",
        hello.klf_version,
        hello.layouts.len()
    );

    match client.push_module(&hello, &module) {
        Ok(Pushed::Installed(replaced)) => {
            out!("Pushed: Ok {module_name} ({replaced} functions, entry {entry})");
            ExitCode::SUCCESS
        }
        Ok(Pushed::Refused(message)) => {
            err!("Pushed: Failed {module_name}: {message}");
            ExitCode::FAILURE
        }
        Err(message) => fail(&message),
    }
}

fn entry_of(rest: &[String]) -> String {
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        if argument == "--entry" {
            return rest.next().cloned().unwrap_or_else(|| "main".to_owned());
        }
    }
    "main".to_owned()
}

/// Compiles one source file to KIR, so what crosses the network is the
/// portable form and the device lowers it with the toolchain it already runs.
fn compile(source: &str, path: &str, module_name: &str) -> Result<klf_kir::Module, String> {
    // A `.kir` file is already the portable form, so it goes across as it
    // stands; anything else goes through the language's frontend first.
    let is_kir = std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("kir"));
    if is_kir {
        return pipeline::parse_kir(source, path);
    }
    let target = klf_driver::host_triple();
    let target = target
        .parse()
        .map_err(|error| format!("invalid host target `{target}`: {error}"))?;
    pipeline::compile(source, path, module_name, target).map_err(|error| error.rendered)
}

fn fail(message: &str) -> ExitCode {
    err!("klf push: {message}");
    ExitCode::FAILURE
}
