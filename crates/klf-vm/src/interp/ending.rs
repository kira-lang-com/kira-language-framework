//! `klf.runtime.at_exit` and `klf.process.exit` on the VM. A function value is
//! a function index, so the machine keeps the index and calls it once as the
//! run ends. An exit ends the run as a return of its status from the entry.

use super::{Machine, Outcome};
use crate::capabilities::Capabilities;

pub(super) const AT_EXIT: &str = "klfsvc_runtime_at_exit";
pub(super) const EXIT: &str = "klfsvc_process_exit";

/// The reasons the native runtime passes, plus one for a fault only the VM
/// can report.
const RETURNED: u64 = 0;
const TRAPPED: u64 = 1;
const FAILED: u64 = 2;
const FAULTED: u64 = 3;

/// Stores the function in the service's one argument, 0 for none, and
/// answers the one it replaces.
pub(super) fn replace(exit_function: &mut Option<u64>, arguments: &[u64]) -> u64 {
    let function = arguments.first().copied().filter(|function| *function != 0);
    std::mem::replace(exit_function, function).unwrap_or(0)
}

fn reason(outcome: &Outcome) -> (u64, u64) {
    match outcome {
        Outcome::Return(value) => (RETURNED, value.unwrap_or(0)),
        Outcome::Fail(value) => (FAILED, *value),
        Outcome::Trap(code) => {
            let number = code
                .rsplit('-')
                .next()
                .and_then(|digits| digits.parse().ok())
                .unwrap_or(0);
            let reason = if code.starts_with("KLF-VM-") {
                FAULTED
            } else {
                TRAPPED
            };
            (reason, number)
        }
    }
}

impl Machine<'_> {
    /// Runs the registered exit function, if any, for how the run ended. A
    /// function that does not return ends the run its own way, as natively.
    pub(super) fn end(&mut self, outcome: Outcome, capabilities: &Capabilities) -> Outcome {
        let Some(function) = self.exit_function.take() else {
            return outcome;
        };
        let Ok(function) = usize::try_from(function) else {
            return Outcome::Trap("KLF-VM-0001".to_owned());
        };
        let (reason, detail) = reason(&outcome);
        match self.call(function, &[reason, detail], capabilities) {
            Outcome::Return(_) => outcome,
            ended => ended,
        }
    }
}
