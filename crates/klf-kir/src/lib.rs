//! KLF's ownership-aware SSA intermediate representation.

mod aggregates;
mod analysis;
mod c_shim;
mod debug;
mod declaration;
mod indirect;
mod model;
mod parse;
mod reach;
mod recursion;
mod syscalls;
mod text;
mod type_parse;
mod unwind;
mod variadic;
mod vectors;
mod witness;
mod witness_calls;

pub use aggregates::aggregates_to_memory;
pub use analysis::{
    ModuleIndex, ValueTypes, infer_value_types, infer_value_types_in, unmanaged_manifest,
};
pub use c_shim::c_shim_source;
pub use indirect::indirect_branches_to_switches;
pub use model::*;
pub use parse::{ParseError, parse_module};
pub use syscalls::linux_syscall;
pub use text::function_text;
pub use unwind::unwinding_without_handlers;
pub use variadic::variadics_to_areas;
pub use vectors::vectors_to_lanes;
pub use witness::WitnessDeclaration;
pub use witness_calls::witness_calls_to_direct;

/// The module in the shape every backend lowers: witness calls on a
/// concrete type made direct, unwinding no handler can stop as traps,
/// indirect branches as switches, RIDL variadics as argument areas, vector
/// values split into lanes, then aggregate values moved into storage. `None` when no rewrite changed anything.
#[must_use]
pub fn legalize(module: &Module) -> Option<Module> {
    let mut current: Option<Module> = None;
    for rewrite in [
        witness_calls_to_direct,
        unwinding_without_handlers,
        indirect_branches_to_switches,
        variadics_to_areas,
        vectors_to_lanes,
        aggregates_to_memory,
    ] {
        if let Some(next) = rewrite(current.as_ref().unwrap_or(module)) {
            current = Some(next);
        }
    }
    current
}

pub mod kirb;
