//! Tier 2 for a backend that reads overflow from the machine's flags and
//! leaves every value to its register allocator (section 14.1).

mod edge;
mod flags;
mod handoff;

use super::Context;
use klf_kir::Function;

pub(super) fn shape(function: &mut Function, context: &Context<'_>) {
    let flagged = flags::machine_flags(function, context);
    let handed = handoff::overflow_paths(function);
    if flagged || handed {
        super::clean::remove_dead(function, context);
    }
}
