//! Host capabilities (section 15.7) and the kernel service bridge (section
//! 8.12). An embedder gives the VM a table of named functions; a running
//! program reaches them by calling the name as a foreign symbol. This is the
//! transport a definition (once written in KLF Kira and executed) uses to call
//! kernel services — resolved against the table here rather than through the
//! process's dynamic symbols.
//!
//! A capability takes the argument registers as machine words and returns a
//! result word, matching the `call.foreign` shape. Capabilities resolve before
//! the dynamic (`dlsym`) path, so a name registered here shadows any libc
//! symbol.

use crate::foreign::CallPlan;
use crate::program::Import;
use crate::program::ImportKind;
use klf_rt::ServiceMemory;
use std::collections::HashMap;

type Handler = Box<dyn Fn(&[u64], Option<&mut dyn ServiceMemory>) -> u64 + Send + Sync>;

pub(crate) struct ResolvedCapabilities<'a> {
    handlers: Vec<Option<&'a Handler>>,
    plans: Vec<Option<CallPlan>>,
}

impl ResolvedCapabilities<'_> {
    pub(crate) fn native_plan(&self, import: usize) -> Option<&CallPlan> {
        self.plans.get(import).and_then(Option::as_ref)
    }

    pub(crate) fn call_with_memory(
        &self,
        import: usize,
        arguments: &[u64],
        memory: &mut dyn ServiceMemory,
    ) -> Option<u64> {
        self.handlers
            .get(import)
            .copied()
            .flatten()
            .map(|handler| handler(arguments, Some(memory)))
    }
}

/// A named set of host functions a program may call.
#[derive(Default)]
pub struct Capabilities {
    table: HashMap<String, Handler>,
}

impl Capabilities {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name` as a callable capability.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        handler: impl Fn(&[u64]) -> u64 + Send + Sync + 'static,
    ) {
        self.table.insert(
            name.into(),
            Box::new(move |arguments, _| handler(arguments)),
        );
    }

    /// Register a capability that can read or write validated execution
    /// buffers during a foreign call.
    pub fn register_with_memory(
        &mut self,
        name: impl Into<String>,
        handler: impl Fn(&[u64], &mut dyn ServiceMemory) -> u64 + Send + Sync + 'static,
    ) {
        self.table.insert(
            name.into(),
            Box::new(move |arguments, memory| {
                memory.map_or(u64::MAX, |memory| handler(arguments, memory))
            }),
        );
    }

    /// Invoke `name` if it is registered, returning its result word.
    #[must_use]
    pub fn call(&self, name: &str, arguments: &[u64]) -> Option<u64> {
        self.table.get(name).map(|handler| handler(arguments, None))
    }

    /// Invoke a capability with the active backend's validated memory.
    #[must_use]
    pub fn call_with_memory(
        &self,
        name: &str,
        arguments: &[u64],
        memory: &mut dyn ServiceMemory,
    ) -> Option<u64> {
        self.table
            .get(name)
            .map(|handler| handler(arguments, Some(memory)))
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    pub(crate) fn resolve_imports<'a>(&'a self, imports: &[Import]) -> ResolvedCapabilities<'a> {
        ResolvedCapabilities {
            handlers: imports
                .iter()
                .map(|import| self.table.get(import.symbol.as_str()))
                .collect(),
            plans: imports
                .iter()
                .map(|import| match &import.kind {
                    ImportKind::Call(signature) | ImportKind::Pointer(signature) => {
                        Some(CallPlan::new(signature))
                    }
                    ImportKind::Variadic { fixed, signature } => {
                        Some(CallPlan::variadic(signature, *fixed as usize))
                    }
                    _ => None,
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}
