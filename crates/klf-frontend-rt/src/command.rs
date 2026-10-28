//! Running one of the definition's `ParsableCommand`s (`KLF-CLI.md` section
//! 4) through the trampoline's command selector.

use crate::klffe_phase;
use klf_defsvc::{ServiceSink, register_command_values};
use klf_native_rt::services;

const COMMAND_SELECTOR: i64 = 8;

/// Runs command `index` with `values`, one list per argument slot, and
/// answers the exit status its `run` returns.
pub(crate) fn run(index: u32, values: Vec<Vec<klf_args::Value>>) -> Result<i32, String> {
    let mut sink = ServiceSink::new();
    register_command_values(values, &mut sink);
    services::install(sink.into_table(), String::new());
    // SAFETY: `klffe_phase` is emitted into this binary with this signature;
    // selector 8 calls the generated `__klf_command` entry with the index.
    let word = unsafe { klffe_phase(COMMAND_SELECTOR, i64::from(index), 0, 0) };
    let reported = services::uninstall();
    if !reported.is_empty() {
        return Err("the command reported a diagnostic".to_owned());
    }
    Ok(i32::try_from(word).unwrap_or(1))
}
