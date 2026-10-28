//! The KLF bytecode VM: lowering of KIR to `.klbc` and a threaded interpreter.
//!
//! The interpreter works at the bit level over fixed-width integers, so the
//! width-narrowing and sign casts below are deliberate. The dispatch loop
//! and the lowering passes are matches over every opcode or operation, which
//! read best whole and with one arm per kind, so `too_many_lines` and
//! `match_same_arms` are allowed. The handlers the dispatch loop calls are
//! `inline(always)` on purpose: they are the interpreter's hot path.
// Android's thread locals are emulated, and the lint misreads the
// `thread_local!` expansion there as a non-`const` initializer.
#![cfg_attr(target_os = "android", allow(clippy::missing_const_for_thread_local))]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::inline_always
)]

mod bytecode;
mod capabilities;
mod child_services;
mod code;
mod crypto_services;
mod file_services;
mod foreign;
mod interp;
mod klbc;
mod link;
mod lower;
mod net_services;
mod numeric;
mod program;
mod services;
mod thread_services;
mod trap_operands;
mod types;

pub use bytecode::{Instr, Opcode};
pub use capabilities::Capabilities;
pub use code::{Code, PushBusy, rebase_imports};
pub use interp::{
    CANCELLED, Cycles, EvaluationBudget, Instance, Machine, Outcome, cancellable, run, run_process,
    run_traced, run_with,
};
pub use klbc::{KlbcError, decode, encode};

/// The path of the loaded native image holding `address`, as a library
/// finds the file it was loaded from.
#[must_use]
pub fn image_of(address: usize) -> Option<String> {
    foreign::image_of(address)
}
pub use klf_diag::RuntimeDiagnostic;
pub use link::{link, link_std, std_bytecode_path};
pub use lower::{
    LowerError, entered_from_c, entry_signature, foreign_class, lower_module, lower_module_traced,
    vm_entry_symbol,
};
pub use program::{
    BcFunction, Const, ForeignClass, ForeignSignature, Import, ImportKind, Program, Traces,
};
pub use services::{
    KernelServicesWithDiagnostics, kernel_services, kernel_services_with_source,
    kernel_services_with_source_and_diagnostics,
};

use klf_kir::Module;

/// Lower a module and run one of its functions with integer arguments.
///
/// # Errors
/// Returns [`LowerError`] when the module cannot be lowered or the entry is
/// missing.
pub fn run_module(module: &Module, entry: &str, arguments: &[u64]) -> Result<Outcome, LowerError> {
    run_module_with_capabilities(module, entry, arguments, &Capabilities::new())
}

/// Evaluates KIR with a private bounded heap and no access to host calls.
/// Initializers and the requested entry consume the same instruction budget.
///
/// # Errors
/// Returns [`LowerError`] when the module does not lower to VM code or has
/// no function named `entry`.
pub fn evaluate_module(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    budget: &mut EvaluationBudget,
) -> Result<Outcome, LowerError> {
    let program = lower_module(module)?;
    let index = program
        .function_index(entry)
        .ok_or_else(|| LowerError(format!("no function named `{entry}`")))?;
    let mut machine = match Machine::for_evaluation(&program, budget) {
        Ok(machine) => machine,
        Err(outcome) => return Ok(outcome),
    };
    for initializer in &program.initializers {
        let outcome = machine.evaluate(*initializer, &[], budget);
        if !matches!(outcome, Outcome::Return(_)) {
            return Ok(outcome);
        }
    }
    Ok(machine.evaluate(index, arguments, budget))
}

/// Lower a module and run one of its functions, resolving `call.foreign` names
/// against `capabilities` first (section 15.7 / 8.12).
///
/// # Errors
/// Returns [`LowerError`] when the module cannot be lowered or the entry is
/// missing.
pub fn run_module_with_capabilities(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    capabilities: &Capabilities,
) -> Result<Outcome, LowerError> {
    let program = lower_module(module)?;
    let index = program
        .function_index(entry)
        .ok_or_else(|| LowerError(format!("no function named `{entry}`")))?;
    Ok(run_with(&program, index, arguments, capabilities))
}
