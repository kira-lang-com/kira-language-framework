//! The framework's own tasks, run through Cargo from anywhere in the checkout:
//!
//! ```sh
//! cargo xtask release OUT VERSION  # this host's packed distribution, as CI publishes it
//! cargo xtask bootstrap-kira       # install KLF, Kira and knvm from this monorepo
//! ```

mod bootstrap;
mod paths;
mod process;
mod release;

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        Some("release") => release::command(&arguments[1..]),
        Some("bootstrap-kira") => bootstrap::command(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage:\n  cargo xtask release OUT VERSION\n  cargo xtask bootstrap-kira";
