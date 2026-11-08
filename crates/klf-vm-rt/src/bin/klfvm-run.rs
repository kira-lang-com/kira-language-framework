//! `<driver>-run`, the runner a sysroot names for a language's bytecode
//! (section 5.4). One binary serves every language: the entry convention a
//! language declares is read at startup, so a sysroot copies this file rather
//! than generating and compiling a `main` per language.
//!
//! The VM is linked in whole, and with it the native runtime, so bytecode can
//! reach every std export by name.

#![allow(unsafe_code)]

use klf_vm_rt::{klfvm_main, klfvm_process_exit_main, klfvm_process_main};
use std::ffi::{CString, OsString, c_char, c_int};
use std::process::ExitCode;

/// The environment variable naming the entry convention, one of `outcome`,
/// `process` or `process-exit`. Absent, the file a sysroot wrote beside this
/// runner says; absent too, a program reports the outcome line the kernel's
/// test drivers read.
const CONVENTION: &str = "KLF_VM_ENTRY_CONVENTION";

/// The entry point a program runs, and how it reports the result.
fn convention() -> String {
    if let Ok(name) = std::env::var(CONVENTION) {
        return name;
    }
    let mut path = std::env::current_exe().unwrap_or_default();
    path.set_extension("convention");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn main() -> ExitCode {
    let mut arguments: Vec<OsString> = std::env::args_os().collect();
    let selected = match select_convention(&mut arguments) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let convention = selected.unwrap_or_else(convention);
    let arguments: Vec<CString> = arguments
        .into_iter()
        .map(|argument| CString::new(argument.to_string_lossy().as_bytes()).unwrap_or_default())
        .collect();
    let count = c_int::try_from(arguments.len()).unwrap_or(c_int::MAX);
    let values: Vec<*const c_char> = arguments.iter().map(|argument| argument.as_ptr()).collect();
    let status = match convention.as_str() {
        "process" => unsafe { klfvm_process_main(count, values.as_ptr()) },
        "process-exit" => unsafe { klfvm_process_exit_main(count, values.as_ptr()) },
        _ => unsafe { klfvm_main(count, values.as_ptr()) },
    };
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}

fn select_convention(arguments: &mut Vec<OsString>) -> Result<Option<String>, String> {
    let Some(value) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        return Ok(None);
    };
    let Some(convention) = value.strip_prefix("--klf-entry-convention=") else {
        return Ok(None);
    };
    if !matches!(convention, "outcome" | "process" | "process-exit") {
        return Err(format!(
            "unsupported bytecode entry convention `{convention}`"
        ));
    }
    let convention = convention.to_owned();
    arguments.remove(1);
    Ok(Some(convention))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runner_selects_its_entry_convention_without_forwarding_the_option() {
        let mut arguments = vec![
            "klfvm-run".into(),
            "--klf-entry-convention=process-exit".into(),
            "app.klbc".into(),
            "hello".into(),
        ];
        assert_eq!(
            select_convention(&mut arguments),
            Ok(Some("process-exit".into()))
        );
        assert_eq!(
            arguments,
            ["klfvm-run", "app.klbc", "hello"].map(OsString::from)
        );
        assert!(
            select_convention(&mut vec![
                "runner".into(),
                "--klf-entry-convention=invalid".into()
            ])
            .is_err()
        );
    }
}
