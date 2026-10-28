//! Cancelling an analysis (`KLF-EDITOR.md` section 8.5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type Callback = Box<dyn Fn() + Send + Sync>;

/// Turns true once when the analysis it was given to is no longer wanted.
/// An analyzer reads it, or registers a callback for the moment it turns.
#[derive(Default)]
pub struct Cancellation {
    flag: Arc<AtomicBool>,
    callbacks: Mutex<Vec<Callback>>,
}

impl Cancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels, running every registered callback once.
    pub fn cancel(&self) {
        if self.flag.swap(true, Ordering::AcqRel) {
            return;
        }
        let callbacks = std::mem::take(
            &mut *self
                .callbacks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for callback in callbacks {
            callback();
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// The flag itself, for code that polls it without this type.
    #[must_use]
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }

    /// Runs `callback` when this is cancelled, or now if it already is.
    pub fn on_cancel(&self, callback: impl Fn() + Send + Sync + 'static) {
        {
            let mut callbacks = self
                .callbacks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !self.is_cancelled() {
                callbacks.push(Box::new(callback));
                return;
            }
        }
        callback();
    }
}
