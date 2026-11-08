//! Route host RIDL callbacks back to the thread running Kira's host VM.

use klf_ridl_wire::Reply;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, PoisonError, mpsc};
use std::time::Duration;

type Task = Box<dyn FnOnce() -> Reply + Send>;

struct Pending {
    task: Task,
    reply: mpsc::SyncSender<Reply>,
}

thread_local! {
    static ACTIVE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct Enter(usize);

impl Enter {
    fn new(callbacks: &Callbacks) -> Self {
        let identity = std::ptr::from_ref(callbacks) as usize;
        ACTIVE.with(|active| active.borrow_mut().push(identity));
        Self(identity)
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let current = active.borrow_mut().pop();
            debug_assert_eq!(current, Some(self.0));
        });
    }
}

/// A rendezvous between a RIDL worker and the host's execution thread.
/// Calls made while already dispatching may nest on that same thread.
#[derive(Default)]
pub(crate) struct Callbacks {
    queue: Mutex<VecDeque<Pending>>,
    ready: Condvar,
    runner: Mutex<()>,
}

impl Callbacks {
    pub(crate) fn invoke(&self, task: Task) -> Reply {
        let same_thread = self.is_owner();
        if same_thread {
            return invoke_task(task);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(Pending { task, reply });
        self.ready.notify_one();
        receiver
            .recv()
            .unwrap_or_else(|_| Reply::Refused("Kira host callback dispatcher stopped".into()))
    }

    /// Executes queued callbacks on the caller's thread, never on an IPC worker.
    pub(crate) fn pump(&self) -> bool {
        if self.is_owner() {
            return self.drain();
        }
        let _runner = match self.runner.try_lock() {
            Ok(runner) => runner,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        self.drain()
    }

    fn drain(&self) -> bool {
        let mut executed = false;
        loop {
            let request = self
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            let Some(request) = request else {
                break;
            };
            executed = true;
            let _enter = Enter::new(self);
            let answer = invoke_task(request.task);
            let _ = request.reply.send(answer);
        }
        executed
    }

    /// Wait briefly for another callback so the host can also poll for exit.
    pub(crate) fn wait_for_work(&self) {
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.is_empty() {
            let _unused = self
                .ready
                .wait_timeout(queue, Duration::from_millis(10))
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Manual `HostedApp::dispatch_next` already runs on the owning thread.
    pub(crate) fn with_owner<T>(&self, action: impl FnOnce() -> T) -> T {
        if self.is_owner() {
            return action();
        }
        let _runner = self.runner.lock().unwrap_or_else(PoisonError::into_inner);
        let _enter = Enter::new(self);
        action()
    }

    fn is_owner(&self) -> bool {
        ACTIVE.with(|active| active.borrow().contains(&(self as *const Self as usize)))
    }
}

fn invoke_task(task: Task) -> Reply {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
        .unwrap_or_else(|_| Reply::Refused("Kira host RIDL callback panicked".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_ridl_wire::Value;
    use std::sync::Arc;

    #[test]
    fn a_faulty_host_callback_does_not_poison_the_ridl_dispatcher() {
        let callbacks = Arc::new(Callbacks::default());
        for fail in [true, false] {
            let thread_callbacks = Arc::clone(&callbacks);
            let worker = std::thread::spawn(move || {
                thread_callbacks.invoke(Box::new(move || {
                    if fail {
                        panic!("faulty Kira host callback");
                    }
                    Reply::Returned(Value::Uint(42))
                }))
            });
            while !worker.is_finished() {
                if !callbacks.pump() {
                    callbacks.wait_for_work();
                }
            }
            let reply = worker.join().unwrap();
            if fail {
                assert!(matches!(reply, Reply::Refused(_)));
            } else {
                assert_eq!(reply, Reply::Returned(Value::Uint(42)));
            }
        }
    }
}
