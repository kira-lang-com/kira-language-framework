//! The mandatory raw-to-canonical KIR passes (section 11.6). Frontends emit raw
//! KIR where owned values are used but not destroyed; the kernel runs these
//! passes to produce canonical KIR whose ownership balances (verifier rule V4).
//!
//! Implemented so far: copy/move resolution and destroy insertion for owned SSA
//! values (`ref`, `uniq`, `weak`). Definite initialization, borrow checking and
//! address-only lowering follow as the model grows the instructions they need.

mod addr_lower;
mod closures;
mod common;
mod destroy;
mod function_abi;
mod initcheck;
mod monomorphize;
mod optimize;
mod resolve;

pub use monomorphize::{monomorphize, monomorphize_owned};
pub use optimize::{Backend, Level, Memo, optimize, optimize_with};

use klf_diag::Diagnostic;
use klf_kir::{Module, Stage};

/// Run the mandatory passes over a raw module and return the canonical module
/// plus any ownership diagnostics.
///
/// The returned module is marked `Stage::Canonical`; callers should not use it
/// when the diagnostics contain errors.
#[must_use]
pub fn canonicalize(module: &Module) -> (Module, Vec<Diagnostic>) {
    let mut result = module.clone();
    let mut diagnostics = Vec::new();
    let index = klf_kir::ModuleIndex::of(module);
    for function in &mut result.functions {
        initcheck::check_initialization(function, &mut diagnostics);
        resolve::resolve_copies(function, &index, &mut diagnostics);
        destroy::insert_destroys(function, &index);
        addr_lower::lower_address_only(function, module, &index);
    }
    result.stage = Stage::Canonical;
    (result, diagnostics)
}
