//! The analysis validation produces (section 16.3).

use crate::consts::ConstValue;
use klf_shader::resolve::Resolution;
use klf_shader::{Function, Handle, Module, Stage};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct ExpressionInfo {
    pub ty: Resolution,
    /// Uses by expressions and statements.
    pub ref_count: u32,
    /// Whether the value may differ between invocations.
    pub non_uniform: bool,
}

/// How a function uses one global.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalUse {
    pub read: bool,
    pub write: bool,
}

impl GlobalUse {
    #[must_use]
    pub const fn any(self) -> bool {
        self.read || self.write
    }
}

#[derive(Clone, Debug, Default)]
pub struct FunctionInfo {
    pub expressions: Vec<ExpressionInfo>,
    /// Indexed by global handle.
    pub globals: Vec<GlobalUse>,
    pub may_kill: bool,
    pub calls: BTreeSet<Handle<Function>>,
    /// Operations only some stages allow, with where they are.
    pub stage_operations: Vec<(StageOperation, klf_shader::Span)>,
    /// Operations that need uniform control flow, found at a non-uniform
    /// point.
    pub non_uniform_operations: Vec<(&'static str, klf_shader::Span)>,
}

/// An operation only some stages allow (SV21).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOperation {
    Kill,
    Derivative,
    ImplicitSample,
    Workgroup,
    Barrier,
}

impl StageOperation {
    #[must_use]
    pub const fn allowed(self, stage: Stage) -> bool {
        match self {
            Self::Kill | Self::Derivative | Self::ImplicitSample => {
                matches!(stage, Stage::Fragment)
            }
            Self::Workgroup | Self::Barrier => matches!(stage, Stage::Compute),
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kill => "kill",
            Self::Derivative => "a derivative",
            Self::ImplicitSample => "an implicit-level sample",
            Self::Workgroup => "workgroup memory",
            Self::Barrier => "a barrier",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    pub global_types: Vec<Resolution>,
    /// The value of every global expression that is constant; `None` for
    /// those that read an override.
    pub constants: Vec<Option<ConstValue>>,
    pub functions: Vec<FunctionInfo>,
    pub entries: Vec<FunctionInfo>,
}

impl ModuleInfo {
    #[must_use]
    pub fn function(&self, which: klf_shader::FunctionRef) -> &FunctionInfo {
        match which {
            klf_shader::FunctionRef::Function(handle) => &self.functions[handle.index()],
            klf_shader::FunctionRef::Entry(index) => &self.entries[index],
        }
    }

    /// The globals an entry point uses, through every function it calls.
    #[must_use]
    pub fn entry_globals(&self, module: &Module, entry: usize) -> Vec<GlobalUse> {
        let mut uses = vec![GlobalUse::default(); module.globals.len()];
        let mut seen = BTreeSet::new();
        let mut stack = vec![&self.entries[entry]];
        while let Some(info) = stack.pop() {
            for (total, used) in uses.iter_mut().zip(&info.globals) {
                total.read |= used.read;
                total.write |= used.write;
            }
            for callee in &info.calls {
                if seen.insert(*callee) {
                    stack.push(&self.functions[callee.index()]);
                }
            }
        }
        uses
    }

    /// Every function an entry point reaches, itself excluded.
    #[must_use]
    pub fn reachable(&self, entry: usize) -> BTreeSet<Handle<Function>> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<Handle<Function>> = self.entries[entry].calls.iter().copied().collect();
        while let Some(function) = stack.pop() {
            if seen.insert(function) {
                stack.extend(self.functions[function.index()].calls.iter().copied());
            }
        }
        seen
    }
}
