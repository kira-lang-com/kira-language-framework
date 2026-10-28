//! A program with a machine of its own that native code calls into (section
//! 17.3): a program run by a runner, or a RIDL module loaded as a plugin
//! beside other programs, each instance with its own heap and globals. Its
//! native library holds the entries C calls its functions through, which
//! the instance binds to itself, so a call from any module reaches the
//! instance that owns the function.
#![allow(unsafe_code)]

use super::{Machine, Outcome, ResolvedCapabilities, execute};
use crate::capabilities::Capabilities;
use crate::program::{ForeignSignature, ImportKind, Program};
use std::cell::UnsafeCell;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The symbol through which a program's native library learns its instance.
const BIND: &str = "klf_vm_bind";

/// A loaded program, its machine, and the host capabilities it runs with.
///
/// Fields drop in order: the machine and the resolved imports borrow the
/// program and the capabilities, which are boxed so neither moves.
pub struct Instance {
    state: UnsafeCell<State>,
    capabilities: Box<Capabilities>,
    program: Box<Program>,
    gate: Gate,
}

struct State {
    resolved: ResolvedCapabilities<'static>,
    machine: Machine<'static>,
}

// SAFETY: the gate admits one thread at a time; calls on that thread nest,
// as a native callback into a running machine always has.
unsafe impl Sync for Instance {}
// SAFETY: nothing in the instance is tied to the thread that created it.
unsafe impl Send for Instance {}

impl Instance {
    /// Loads the native libraries `program` imports, each path relative to
    /// `base`, and binds them to a new instance of the program, which runs
    /// nothing until [`Self::initialize`].
    ///
    /// # Errors
    /// Returns why a library could not be loaded.
    pub fn load(
        program: Program,
        base: &Path,
        capabilities: Capabilities,
    ) -> Result<Box<Self>, String> {
        let mut libraries = Vec::new();
        let mut firsts = Vec::new();
        for import in &program.imports {
            if let ImportKind::Library { first } = import.kind {
                libraries.push(crate::foreign::load(
                    &base.join(&import.symbol).to_string_lossy(),
                )?);
                firsts.push(first);
            }
        }
        let program = Box::new(program);
        let capabilities = Box::new(capabilities);
        // SAFETY: both boxes live in the instance beside the state that
        // borrows them, are never replaced, and drop after it.
        let (shared_program, shared_capabilities): (&'static Program, &'static Capabilities) = unsafe {
            (
                &*std::ptr::from_ref(program.as_ref()),
                &*std::ptr::from_ref(capabilities.as_ref()),
            )
        };
        let mut machine = Machine::new(shared_program);
        machine.libraries.clone_from(&libraries);
        let instance = Box::new(Self {
            state: UnsafeCell::new(State {
                resolved: shared_capabilities.resolve_imports(&shared_program.imports),
                machine,
            }),
            capabilities,
            program,
            gate: Gate::default(),
        });
        for (library, first) in libraries.into_iter().zip(firsts) {
            if let Some(bind) = crate::foreign::resolve(&[library], BIND) {
                // SAFETY: every program's native library defines its bind
                // function as `void klf_vm_bind(const void *, uint32_t)`.
                let bind: extern "C" fn(*const Self, u32) = unsafe { std::mem::transmute(bind) };
                bind(std::ptr::from_ref(instance.as_ref()), first);
            }
        }
        Ok(instance)
    }

    /// Gives each function C calls a native entry from the VM's own pool
    /// (section 17.3) when no library the program loaded defines one. This is
    /// how a device runs lowered KIR with no native half built beside it.
    /// `signatures` names each function C calls with the C signature it is
    /// called with. Returns the names left without an entry.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn provide_entries(&self, signatures: &[(String, ForeignSignature)]) -> Vec<String> {
        self.with(|machine, _, _| {
            let mut unbound = Vec::new();
            for (name, signature) in signatures {
                let symbol = crate::vm_entry_symbol(name);
                let Some(index) = machine.program.imports.iter().position(|import| {
                    import.kind == ImportKind::Address && import.symbol == symbol
                }) else {
                    continue;
                };
                if machine.resolved[index] != 0
                    || crate::foreign::resolve(&machine.libraries, &symbol).is_some()
                {
                    continue;
                }
                let entry = machine
                    .function_index(name)
                    .and_then(|function| u32::try_from(function).ok())
                    .and_then(|function| super::entries::bind(self, name, function, signature));
                match entry {
                    Some(address) => machine.resolved[index] = address,
                    None => unbound.push(name.clone()),
                }
            }
            unbound
        })
    }

    /// Points the native entries [`Self::provide_entries`] bound at their
    /// functions in the code the next call enters, after a push that may have
    /// moved them.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn retarget_entries(&self) {
        super::entries::retarget(self, |name| {
            self.function_index(name)
                .and_then(|index| u32::try_from(index).ok())
        });
    }

    /// Runs the program's initializers. One that does not return ends the
    /// program, through the exit function it registered, with its outcome.
    pub fn initialize(&self) -> Outcome {
        self.with(|machine, resolved, capabilities| {
            let program = machine.program;
            for initializer in &program.initializers {
                let outcome =
                    execute::<false>(machine, *initializer, &[], capabilities, resolved, None);
                if !matches!(outcome, Outcome::Return(_)) {
                    return machine.end(outcome, capabilities);
                }
            }
            Outcome::Return(None)
        })
    }

    #[must_use]
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// The index of `name` in the code the next call enters, which is the
    /// version a push just installed (section 17.5).
    #[must_use]
    pub fn function_index(&self, name: &str) -> Option<usize> {
        self.with(|machine, _, _| machine.function_index(name))
    }

    /// Replaces the code the next call enters with `code`, keeping the globals
    /// and heap this instance already holds (section 17.5).
    ///
    /// # Errors
    /// Returns [`PushBusy`] when both versions exist and the one this would
    /// retire still has frames running.
    pub fn adopt(&self, code: Arc<crate::Code>) -> Result<u8, crate::PushBusy> {
        self.with(|machine, _, _| machine.adopt(code))
    }

    /// Drops a retired version no frame is running.
    pub fn reap(&self) {
        self.with(|machine, _, _| machine.reap());
    }

    /// Whether a frame entered under `version` is still running, which decides
    /// whether that version can be dropped.
    #[must_use]
    pub fn version_busy(&self, version: u8) -> bool {
        self.with(|machine, _, _| machine.version_busy(version))
    }

    /// The code version the next call enters.
    #[must_use]
    pub fn version(&self) -> u8 {
        self.with(|machine, _, _| machine.version())
    }

    /// How many parameters `function` takes in the code the next call
    /// enters.
    #[must_use]
    pub fn parameter_count(&self, function: usize) -> usize {
        self.with(|machine, _, _| super::reentry::parameter_count(machine.program(), function))
    }

    /// Runs `entry` with `arguments` on the instance's machine.
    pub fn call(&self, entry: usize, arguments: &[u64]) -> Outcome {
        self.with(|machine, resolved, capabilities| {
            execute::<false>(machine, entry, arguments, capabilities, resolved, None)
        })
    }

    /// Runs `entry` as a C process's `main`, as [`super::run_process`] does,
    /// and then the exit function the program registered.
    pub fn run_process(
        &self,
        entry: usize,
        arguments: &[String],
        environment: &[String],
    ) -> Outcome {
        self.with(|machine, resolved, capabilities| {
            let vector = machine.host_strings(arguments);
            let environment = machine.host_strings(environment);
            let count = u64::try_from(arguments.len()).unwrap_or(0);
            let words = [count, vector, environment];
            let taken = (machine.program.functions[entry].param_count as usize).min(words.len());
            let outcome = execute::<false>(
                machine,
                entry,
                &words[..taken],
                capabilities,
                resolved,
                None,
            );
            machine.end(outcome, capabilities)
        })
    }

    /// Runs `entry` as [`super::run_with`] does: a clean return that leaves
    /// blocks live traps with `KLF-RT-0100`.
    pub fn run(&self, entry: usize, arguments: &[u64]) -> Outcome {
        self.with(|machine, resolved, capabilities| {
            let mut outcome =
                execute::<false>(machine, entry, arguments, capabilities, resolved, None);
            if matches!(outcome, Outcome::Return(_)) && !machine.balanced() {
                outcome = Outcome::Trap("KLF-RT-0100".to_owned());
            }
            machine.end(outcome, capabilities)
        })
    }

    fn with<T>(
        &self,
        body: impl FnOnce(&mut Machine<'static>, &ResolvedCapabilities<'static>, &Capabilities) -> T,
    ) -> T {
        self.gate.enter(|| {
            let state = self.state.get();
            // SAFETY: the gate holds every other thread out. A call nested on
            // this thread comes from native code the machine called, which
            // the machine does not touch until that code returns.
            unsafe {
                body(
                    &mut (*state).machine,
                    &(*state).resolved,
                    &self.capabilities,
                )
            }
        })
    }
}

/// Admits one thread at a time into an instance; the thread inside may
/// enter again, since native code its machine calls may call back.
#[derive(Default)]
struct Gate {
    lock: Mutex<()>,
    owner: AtomicUsize,
}

impl Gate {
    fn enter<T>(&self, body: impl FnOnce() -> T) -> T {
        thread_local! {
            static TOKEN: u8 = const { 0 };
        }
        let token = TOKEN.with(|token| std::ptr::from_ref(token) as usize);
        if self.owner.load(Ordering::Acquire) == token {
            return body();
        }
        let _held = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.owner.store(token, Ordering::Release);
        let _leave = Leave(&self.owner);
        body()
    }
}

/// Clears the gate's owner as the thread inside leaves, by return or unwind.
struct Leave<'a>(&'a AtomicUsize);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Instance {
    fn drop(&mut self) {
        super::entries::release(self);
    }
}
