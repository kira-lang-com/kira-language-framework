//! One version of a module's compiled code (section 17.5).
//!
//! A device holds two: the version new calls enter, and the one frames already
//! running keep finishing in. Each owns everything an execution of that code
//! needs, so a version stays valid for as long as any frame names it. That is
//! what lets a push replace code without stopping the world.

use crate::interp::decoded::{DecodedFunction, decode_program};
use crate::program::Program;

/// Only two code versions exist at once (section 17.5), so a push arriving
/// while both slots are held and the one it would retire is still running
/// cannot be installed. The caller waits for `Machine::version_busy` to go
/// false and retries; a device NACKs with `OLD_CODE_BUSY` when it gives up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PushBusy {
    /// The version the push wanted and could not take.
    pub incoming: u8,
}

impl std::fmt::Display for PushBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "code version {} is still running", self.incoming)
    }
}

impl std::error::Error for PushBusy {}

/// Points `program`'s foreign call sites at the same imports in `running`, the
/// version whose import table the machine resolved, and gives `program` that
/// table. A machine resolves each import once, by its index in the program it
/// loaded, so a pushed version must call its natives through those indices
/// rather than through its own (section 17.5: new calls enter the new code,
/// against the natives already bound).
///
/// # Errors
/// Returns the symbol of a native `program` calls that `running` never
/// imported, which nothing has resolved: adopting it needs a restart.
pub fn rebase_imports(mut program: Program, running: &Program) -> Result<Program, String> {
    let rebased: Vec<Result<u32, String>> = program
        .imports
        .iter()
        .map(|import| {
            running
                .imports
                .iter()
                .position(|known| known == import)
                .and_then(|index| u32::try_from(index).ok())
                .ok_or_else(|| import.symbol.clone())
        })
        .collect();
    for function in &mut program.functions {
        for site in &mut function.call_sites {
            if let Some(index) = site.foreign.as_mut() {
                let target = rebased
                    .get(*index as usize)
                    .cloned()
                    .unwrap_or_else(|| Err(format!("import {index}")));
                *index = target.map_err(|symbol| {
                    format!("the pushed code calls `{symbol}`, which the running program does not import")
                })?;
            }
        }
    }
    program.imports.clone_from(&running.imports);
    Ok(program)
}

/// A module's bytecode at one version: the program and its predecoded
/// functions. Decoding is the expensive part of getting a push onto a device,
/// so it happens once per version here rather than per call.
pub struct Code {
    program: Program,
    decoded: Vec<DecodedFunction>,
}

impl Code {
    /// Predecodes `program` into the form execution runs.
    #[must_use]
    pub fn new(program: Program) -> Self {
        let decoded = decode_program(&program);
        Self { program, decoded }
    }

    #[must_use]
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// The predecoded functions, indexed as the program indexes them.
    pub(crate) fn decoded(&self) -> &[DecodedFunction] {
        &self.decoded
    }

    /// The names this version defines, for the replaced-function count a push
    /// acknowledges.
    #[must_use]
    pub fn function_names(&self) -> Vec<&str> {
        self.program
            .functions
            .iter()
            .map(|function| function.name.as_str())
            .collect()
    }

    #[must_use]
    pub fn function_index(&self, name: &str) -> Option<usize> {
        self.program.function_index(name)
    }
}

impl std::fmt::Debug for Code {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Code")
            .field("functions", &self.program.functions.len())
            .finish_non_exhaustive()
    }
}
