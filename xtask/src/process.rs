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
    let program = program.as_ref();
    let arguments: Vec<_> = arguments.into_iter().map(|argument| argument.as_ref().to_owned()).collect();
    let shown: Vec<_> = arguments.iter().map(|argument| argument.to_string_lossy()).collect();
    println!("   {} {}", program.to_string_lossy(), shown.join(" "));
    let status = Command::new(program)
        .args(&arguments)
        .current_dir(directory)
        .status()
        .map_err(|error| format!("`{}` did not start: {error}", program.to_string_lossy()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` failed ({status})", program.to_string_lossy()))
    }
}

