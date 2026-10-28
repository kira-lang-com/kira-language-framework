//! `klf dist --out <dir> [--targets <t1,t2,...>]`: lay out a KLF distribution
//! (section 5.1) whose `bin/klf` builds toolchains without Rust.

use std::path::Path;
use std::process::{Command, ExitCode};

pub(crate) fn command(args: &[String]) -> ExitCode {
    let mut out = None;
    let mut targets = vec![klf_driver::host_triple().to_owned()];
    let mut timings = false;
    let mut rest = args.iter().peekable();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--timings" => timings = true,
            "--out" => match rest.next() {
                Some(value) => out = Some(value.clone()),
                None => return fail("`--out` needs a directory"),
            },
            "--targets" => match rest.next() {
                Some(value) => {
                    targets = value
                        .split(',')
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                None => return fail("`--targets` needs at least one triple"),
            },
            other => return fail(&format!("klf dist: unexpected argument `{other}`")),
        }
    }
    let Some(out) = out else {
        return fail("usage: klf dist --out DIR [--targets T1,T2] [--timings]");
    };
    let mut finish = klf_diag::status::begin(&format!("Distributing to {out}"), timings);
    let built = klf_driver::build_distribution(Path::new(&out), &targets)
        .and_then(|()| vscode_client(Path::new(&out)));
    finish.end();
    match built {
        Ok(()) => {
            klf_diag::out!("Built: Ok {out}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            klf_diag::out!("Built: Failed");
            fail(&message)
        }
    }
}

/// Builds the VS Code client from `editors/vscode-client` with npm and
/// ships it as `share/klf/editor/vscode-client.js` (`KLF-EDITOR.md` section
/// 19.5, DE-010). It is the one artifact made with Node.js, at release time
/// only; `klf editor gen` puts it in every generated `.vsix`.
fn vscode_client(out: &Path) -> Result<(), String> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editors/vscode-client");
    let npm = |arguments: &[&str]| {
        // On Windows npm is a `.cmd` script, which `Command` finds only by
        // its full name.
        let status = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
            .args(arguments)
            .current_dir(&source)
            .status()
            .map_err(|error| {
                format!("the VS Code client is built with npm, which did not start: {error}")
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "`npm {}` failed in {}",
                arguments.join(" "),
                source.display()
            ))
        }
    };
    if !source.join("node_modules").exists() {
        npm(&["ci", "--no-audit", "--no-fund"])?;
    }
    npm(&["run", "build"])?;
    let shipped = out.join("share/klf/editor");
    std::fs::create_dir_all(&shipped).map_err(|error| error.to_string())?;
    std::fs::copy(
        source.join("dist/vscode-client.js"),
        shipped.join("vscode-client.js"),
    )
    .map_err(|error| format!("copying the VS Code client: {error}"))?;
    Ok(())
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("klf: {message}");
    ExitCode::FAILURE
}
