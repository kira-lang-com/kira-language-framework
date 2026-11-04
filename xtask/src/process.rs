//! Running the tools a task drives, each echoed before it runs.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

pub fn step(title: &str) {
    println!("\n== {title}");
}

/// Runs `program` in `directory`, failing with its name when it does not exit 0.
pub fn run<I, S>(program: impl AsRef<OsStr>, arguments: I, directory: &Path) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_configured(program, arguments, directory, &[], &[])
}

/// Runs a tool with a small set of environment overrides.
pub fn run_with_env<I, S>(
    program: impl AsRef<OsStr>,
    arguments: I,
    directory: &Path,
    environment: &[(&str, &OsStr)],
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_configured(program, arguments, directory, environment, &[])
}

/// Runs a tool after removing selected inherited environment variables.
pub fn run_without_env<I, S>(
    program: impl AsRef<OsStr>,
    arguments: I,
    directory: &Path,
    removed: &[&str],
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_configured(program, arguments, directory, &[], removed)
}

fn run_configured<I, S>(
    program: impl AsRef<OsStr>,
    arguments: I,
    directory: &Path,
    environment: &[(&str, &OsStr)],
    removed: &[&str],
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = program.as_ref();
    let arguments: Vec<_> = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect();
    let shown: Vec<_> = arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect();
    println!("   {} {}", program.to_string_lossy(), shown.join(" "));
    let mut command = Command::new(program);
    command.args(&arguments).current_dir(directory);
    for name in removed {
        command.env_remove(name);
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("`{}` did not start: {error}", program.to_string_lossy()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` failed ({status})", program.to_string_lossy()))
    }
}
