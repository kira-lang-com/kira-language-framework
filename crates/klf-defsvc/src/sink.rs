//! The service table a definition's phase code calls into.
//!
//! The VM looks a capability up by name at every `call.foreign`; a natively
//! compiled frontend resolves the same names once, at the `extern "C"` shim.
//! Both take the handler from here, so the two paths cannot drift.

use klf_rt::ServiceMemory;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type WordHandler = Arc<dyn Fn(&[u64]) -> u64 + Send + Sync>;
type MemoryHandler = Arc<dyn Fn(&[u64], &mut dyn ServiceMemory) -> u64 + Send + Sync>;

/// One service implementation, usable with or without backend memory access.
#[derive(Clone)]
pub struct Handler {
    word: WordHandler,
    memory: Option<MemoryHandler>,
}

impl Handler {
    /// Calls the word-only behavior, or the unavailable sentinel for a
    /// memory-only service.
    #[must_use]
    pub fn call(&self, arguments: &[u64]) -> u64 {
        (self.word)(arguments)
    }

    /// Calls with backend-owned memory when the service requires it; existing
    /// word-only handlers continue to ignore the memory context.
    #[must_use]
    pub fn call_with_memory(&self, arguments: &[u64], memory: &mut dyn ServiceMemory) -> u64 {
        self.memory.as_ref().map_or_else(
            || (self.word)(arguments),
            |handler| handler(arguments, memory),
        )
    }
}

/// A name-to-handler table under construction.
#[derive(Default)]
pub struct ServiceSink {
    table: HashMap<String, Handler>,
}

impl ServiceSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        name: impl Into<String>,
        handler: impl Fn(&[u64]) -> u64 + Send + Sync + 'static,
    ) {
        self.table.insert(
            name.into(),
            Handler {
                word: Arc::new(handler),
                memory: None,
            },
        );
    }

    /// Registers a service that receives bounded access to backend-owned
    /// buffers in addition to its machine-word arguments.
    pub fn register_with_memory(
        &mut self,
        name: impl Into<String>,
        handler: impl Fn(&[u64], &mut dyn ServiceMemory) -> u64 + Send + Sync + 'static,
    ) {
        self.table.insert(
            name.into(),
            Handler {
                word: Arc::new(|_| u64::MAX),
                memory: Some(Arc::new(handler)),
            },
        );
    }

    /// Registers a service that runs against shared builder state.
    ///
    /// A poisoned lock means an earlier service panicked partway through a
    /// build, so there is nothing left to build against. The definition ABI
    /// has no channel for that, so it comes back as the error handle.
    pub fn register_state<S, F>(&mut self, name: impl Into<String>, state: &Arc<Mutex<S>>, body: F)
    where
        S: Send + 'static,
        F: Fn(&mut S, &[u64]) -> u64 + Send + Sync + 'static,
    {
        let state = Arc::clone(state);
        self.register(name, move |arguments| {
            state
                .lock()
                .map_or(u64::MAX, |mut state| body(&mut state, arguments))
        });
    }

    /// Registers a memory-aware service against shared state.
    pub fn register_state_with_memory<S, F>(
        &mut self,
        name: impl Into<String>,
        state: &Arc<Mutex<S>>,
        body: F,
    ) where
        S: Send + 'static,
        F: Fn(&mut S, &[u64], &mut dyn ServiceMemory) -> u64 + Send + Sync + 'static,
    {
        let state = Arc::clone(state);
        self.register_with_memory(name, move |arguments, memory| {
            state
                .lock()
                .map_or(u64::MAX, |mut state| body(&mut state, arguments, memory))
        });
    }

    /// Invokes `name` if it is registered.
    #[must_use]
    pub fn call(&self, name: &str, arguments: &[u64]) -> Option<u64> {
        self.table.get(name).map(|handler| handler.call(arguments))
    }

    #[must_use]
    pub fn into_table(self) -> HashMap<String, Handler> {
        self.table
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}
