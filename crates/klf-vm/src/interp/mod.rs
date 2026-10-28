//! Threaded-dispatch interpreter over compiled bytecode (section 15.6).

mod access;
mod cycles;
pub(crate) mod decoded;
mod ending;
#[cfg(not(target_arch = "wasm32"))]
mod entries;
mod execute;
mod frames;
mod instance;
#[cfg(feature = "execution-profile")]
mod profile;
#[cfg(not(target_arch = "wasm32"))]
mod reentry;
mod step;

use crate::capabilities::{Capabilities, ResolvedCapabilities};
use crate::program::{ImportKind, Program};
pub use cycles::{Cycles, run_traced};
use decoded::{DecodedFunction, decode_program};
use execute::execute;
use frames::FramePool;
pub use instance::Instance;
use klf_rt::Heap;

/// The result of running a function to a terminator.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Return(Option<u64>),
    Fail(u64),
    Trap(String),
}

/// The recursion limit: frames live on the heap, so it bounds memory, not
/// the native stack.
const MAX_DEPTH: usize = 1 << 18;

/// The trap of a run whose cancellation flag turned (`KLF-EDITOR.md`
/// section 8.6): the machine stops at its next call or loop back edge and
/// unwinds its frames.
pub const CANCELLED: &str = "KLF-VM-0030";

thread_local! {
    static AMBIENT: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `body` with `flag` as the cancellation of every machine it creates,
/// so a language server can cancel definition phases that run on the VM.
pub fn cancellable<R>(
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    body: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<std::sync::Arc<std::sync::atomic::AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            AMBIENT.with(|ambient| *ambient.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(AMBIENT.with(|ambient| ambient.borrow_mut().replace(flag)));
    body()
}

/// A budget shared by every entry call in one compile-time evaluation.
/// Memory includes allocator backing storage. Foreign calls are always denied.
pub struct EvaluationBudget {
    pub steps_remaining: u64,
    pub memory_bytes: usize,
}

/// Run `entry` in `program` with the given integer arguments.
///
/// A clean return with a non-empty heap fails the run with `KLF-RT-0100`
/// (section 15.6 heap-balance check).
#[must_use]
pub fn run(program: &Program, entry: usize, arguments: &[u64]) -> Outcome {
    run_with(program, entry, arguments, &Capabilities::new())
}

/// Run `entry`, resolving `call.foreign` names first against `capabilities`
/// (section 15.7 host capabilities / section 8.12 kernel services) and only then
/// against the process's dynamic symbols.
#[must_use]
pub fn run_with(
    program: &Program,
    entry: usize,
    arguments: &[u64],
    capabilities: &Capabilities,
) -> Outcome {
    let mut machine = Machine::new(program);
    for initializer in &program.initializers {
        let outcome = machine.call(*initializer, &[], capabilities);
        if !matches!(outcome, Outcome::Return(_)) {
            return machine.end(outcome, capabilities);
        }
    }
    let mut outcome = machine.call(entry, arguments, capabilities);
    if matches!(outcome, Outcome::Return(_)) && !machine.balanced() {
        outcome = Outcome::Trap("KLF-RT-0100".to_owned());
    }
    machine.end(outcome, capabilities)
}

/// Runs `entry` as a C process's `main`: after the initializers it gets
/// `argc`, `argv` and `envp` (as many as it takes), with the strings copied
/// into the program's heap. A process may exit with blocks still live, so
/// there is no heap-balance check.
#[must_use]
pub fn run_process(
    program: &Program,
    entry: usize,
    arguments: &[String],
    environment: &[String],
    capabilities: &Capabilities,
) -> Outcome {
    let mut machine = Machine::new(program);
    for initializer in &program.initializers {
        let outcome = machine.call(*initializer, &[], capabilities);
        if !matches!(outcome, Outcome::Return(_)) {
            return machine.end(outcome, capabilities);
        }
    }
    let vector = machine.host_strings(arguments);
    let environment = machine.host_strings(environment);
    let count = u64::try_from(arguments.len()).unwrap_or(0);
    let words = [count, vector, environment];
    let taken = (program.functions[entry].param_count as usize).min(words.len());
    let outcome = machine.call(entry, &words[..taken], capabilities);
    machine.end(outcome, capabilities)
}

/// A program with a heap that outlives a single entry call.
///
/// A definition's phases run as separate entry points over one machine, so a
/// value one phase allocates is still addressable in the next. The heap-balance
/// check of 15.6 therefore belongs to the machine, not to each call.
pub struct Machine<'a> {
    program: &'a Program,
    decoded: Vec<DecodedFunction>,
    /// Which code version this machine runs (section 17.5). A frame records
    /// the version it entered, so a swap between entry calls never disturbs a
    /// frame already running the old one. Version 0 is the borrowed program;
    /// 1 is a pushed [`Code`](crate::Code).
    version: u8,
    /// The pushed versions, held so a frame's code outlives the call that
    /// entered it. Only one is ever set at a time; the other is empty or the
    /// version being retired.
    slots: [Option<std::sync::Arc<crate::Code>>; 2],
    /// Frames that entered under each version and have not returned.
    running: [u32; 2],
    frame_pool: FramePool,
    heap: Heap,
    globals: Vec<u64>,
    exit_function: Option<u64>,
    /// The native address of each import once resolved, or 0.
    resolved: Vec<usize>,
    /// The handles of the native libraries the program brought, searched
    /// for its imports before the rest of the process.
    libraries: Vec<usize>,
    /// When set and true, execution stops with [`CANCELLED`].
    cancellation: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The trace maps a counted allocation registers (section 18.6).
    traces: Option<&'a crate::program::Traces>,
    #[cfg(feature = "execution-profile")]
    profile: profile::Profile,
}

impl<'a> Machine<'a> {
    #[must_use]
    pub fn new(program: &'a Program) -> Self {
        let decoded = decode_program(program);
        let mut heap = Heap::new();
        if program
            .imports
            .iter()
            .any(|import| matches!(import.kind, ImportKind::Library { .. }))
        {
            heap.admit_native_memory();
        }
        let globals = allocate_globals(&program.globals, &mut heap);
        Self {
            program,
            decoded,
            version: 0,
            slots: [None, None],
            running: [0; 2],
            frame_pool: Vec::new(),
            heap,
            globals,
            exit_function: None,
            resolved: vec![0; program.imports.len()],
            libraries: Vec::new(),
            cancellation: AMBIENT.with(|ambient| ambient.borrow().clone()),
            traces: None,
            #[cfg(feature = "execution-profile")]
            profile: profile::Profile::new(program),
        }
    }

    /// Creates a private evaluation heap, checking globals before allocation.
    ///
    /// # Errors
    /// Traps with `KLF-VM-0023` when the globals do not fit in the budget.
    pub fn for_evaluation(
        program: &'a Program,
        budget: &EvaluationBudget,
    ) -> Result<Self, Outcome> {
        let decoded = decode_program(program);
        let mut heap = Heap::new();
        let mut globals = Vec::with_capacity(program.globals.len());
        for spec in &program.globals {
            if !heap.allocation_fits(
                spec.size.max(1) as usize,
                spec.align.max(1) as usize,
                budget.memory_bytes,
            ) {
                return Err(Outcome::Trap("KLF-VM-0023".to_owned()));
            }
            globals.extend(allocate_globals(std::slice::from_ref(spec), &mut heap));
        }
        Ok(Self {
            program,
            decoded,
            version: 0,
            slots: [None, None],
            running: [0; 2],
            frame_pool: Vec::new(),
            heap,
            globals,
            exit_function: None,
            resolved: vec![0; program.imports.len()],
            libraries: Vec::new(),
            cancellation: AMBIENT.with(|ambient| ambient.borrow().clone()),
            traces: None,
            #[cfg(feature = "execution-profile")]
            profile: profile::Profile::new(program),
        })
    }

    /// The code version this machine's next entry call runs (section 17.5).
    #[must_use]
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Whether a frame entered under `version` may still be running, which is
    /// what decides whether that version can be dropped.
    #[must_use]
    pub fn version_busy(&self, version: u8) -> bool {
        self.running[usize::from(version)] > 0
    }

    /// Adopts `code` as the version the next entry call enters and returns the
    /// version number it now carries.
    ///
    /// The version being retired stays in its slot until [`Self::reap`] drops
    /// it, so frames that entered it finish in the code they started in. Only
    /// two versions exist at once (section 17.5); installing a third while an
    /// older one is still busy fails and the caller retries.
    ///
    /// Globals keep their addresses and values: this replaces code only, so
    /// every value a running program already holds stays where it is.
    ///
    /// # Errors
    /// Returns [`crate::PushBusy`] when the older version still has frames
    /// running in it.
    pub fn adopt(&mut self, code: std::sync::Arc<crate::Code>) -> Result<u8, crate::PushBusy> {
        let incoming = usize::from(self.version == 0);
        let occupied = usize::from(self.version);
        if self.slots[occupied].is_some() && self.version_busy(incoming as u8) {
            return Err(crate::PushBusy {
                incoming: incoming as u8,
            });
        }
        self.slots[incoming] = Some(code);
        self.version = incoming as u8;
        Ok(self.version)
    }

    /// Drops a retired version no frame is running, freeing its decoded code.
    pub fn reap(&mut self) {
        let current = usize::from(self.version);
        for slot in 0..2 {
            if slot != current && self.running[slot] == 0 {
                self.slots[slot] = None;
            }
        }
    }

    /// The program the next entry call runs, which is the current version's.
    #[must_use]
    pub fn program(&self) -> &Program {
        self.version_code(self.version).0
    }

    /// The index of `name` in the code the next entry call runs, so a caller
    /// with no entry index in hand (a device taking a name off the wire) can
    /// still find it in the version a push just installed.
    #[must_use]
    pub fn function_index(&self, name: &str) -> Option<usize> {
        self.program().function_index(name)
    }

    /// The program and decoded functions of `version`.
    pub(super) fn version_code(&self, version: u8) -> (&Program, &[DecodedFunction]) {
        match &self.slots[usize::from(version)] {
            Some(code) => (code.program(), code.decoded()),
            None => (self.program, &self.decoded),
        }
    }

    fn enter(&mut self, version: u8) {
        self.running[usize::from(version)] += 1;
    }

    fn leave(&mut self, version: u8) {
        self.running[usize::from(version)] = self.running[usize::from(version)].saturating_sub(1);
    }

    /// Executes KIR with no host access, consuming the caller's shared budget.
    pub fn evaluate(
        &mut self,
        entry: usize,
        arguments: &[u64],
        budget: &mut EvaluationBudget,
    ) -> Outcome {
        let capabilities = Capabilities::new();
        let resolved = capabilities.resolve_imports(&self.program.imports);
        execute::<true>(
            self,
            entry,
            arguments,
            &capabilities,
            &resolved,
            Some(budget),
        )
    }

    /// Allocates `size` bytes the evaluated code writes its result into.
    ///
    /// # Errors
    /// Traps with `KLF-VM-0023` when the slot does not fit in the budget.
    pub fn evaluation_slot(
        &mut self,
        size: usize,
        align: usize,
        budget: &EvaluationBudget,
    ) -> Result<u64, Outcome> {
        if !self
            .heap
            .allocation_fits(size.max(1), align.max(1), budget.memory_bytes)
        {
            return Err(Outcome::Trap("KLF-VM-0023".to_owned()));
        }
        Ok(self.heap.alloc_untracked(size.max(1), align.max(1)) as u64)
    }

    pub fn evaluation_read(&self, address: u64, bytes: &mut [u8]) -> bool {
        self.heap.read_buffer(address as *const u8, bytes)
    }

    pub fn evaluation_global(&self, address: u64) -> Option<usize> {
        self.globals.iter().position(|global| *global == address)
    }

    /// Runs one entry point against the machine's heap.
    pub fn call(
        &mut self,
        entry: usize,
        arguments: &[u64],
        capabilities: &Capabilities,
    ) -> Outcome {
        let resolved = capabilities.resolve_imports(&self.program.imports);
        execute::<false>(self, entry, arguments, capabilities, &resolved, None)
    }

    /// A NULL-terminated array of NUL-terminated copies of `strings` in the
    /// machine's heap, as C passes `argv` and `envp`.
    pub fn host_strings(&mut self, strings: &[String]) -> u64 {
        let table = self.heap.alloc_untracked((strings.len() + 1) * 8, 8);
        for (index, string) in strings.iter().enumerate() {
            let bytes = string.as_bytes();
            let copy = self.heap.alloc_untracked(bytes.len() + 1, 1);
            for (offset, byte) in bytes.iter().chain(std::iter::once(&0)).enumerate() {
                self.heap
                    .write(copy.wrapping_add(offset), 1, u64::from(*byte));
            }
            self.heap
                .write(table.wrapping_add(index * 8), 8, copy as u64);
        }
        self.heap.write(table.wrapping_add(strings.len() * 8), 8, 0);
        table as u64
    }

    /// Stops every later run on this machine with [`CANCELLED`] at its next
    /// call or loop back edge once `flag` turns true.
    pub fn cancel_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.cancellation = Some(flag);
    }

    /// Whether every tracked block allocated on this machine has been freed.
    #[must_use]
    pub fn balanced(&self) -> bool {
        self.heap.live_blocks() == 0
    }
}

fn allocate_globals(specs: &[crate::program::GlobalSpec], heap: &mut Heap) -> Vec<u64> {
    let mut addresses = Vec::with_capacity(specs.len());
    for spec in specs {
        let size = spec.size.max(1) as usize;
        let pointer = heap.alloc_untracked(size, spec.align.max(1) as usize);
        let base = pointer as usize;
        let mut written = 0usize;
        while written < size {
            let chunk = u32::try_from(size - written).unwrap_or(8).min(8);
            heap.write((base + written) as *mut u8, chunk, 0);
            written += chunk as usize;
        }
        for (index, byte) in spec.init.iter().enumerate() {
            if index >= size {
                break;
            }
            heap.write((base + index) as *mut u8, 1, u64::from(*byte));
        }
        addresses.push(pointer as u64);
    }
    addresses
}

/// The import a call site calls natively: its own for a `call.foreign`, and
/// for a `call.indirect` through a C function pointer only when its target
/// is not one of the program's bytecode functions.
pub(super) fn native_import(
    program: &Program,
    site: crate::program::CallSite,
    registers: &[u64],
) -> Option<u32> {
    let import = site.foreign?;
    match site.target {
        Some(register)
            if registers
                .get(register as usize)
                .is_some_and(|&target| (target as usize) < program.functions.len()) =>
        {
            None
        }
        _ => Some(import),
    }
}

/// Calls the service or symbol `site` names and writes its result back, or
/// answers how the run ends when the call ends it.
fn foreign_call(
    machine: &mut Machine,
    site: crate::program::CallSite,
    registers: &mut [u64],
    capabilities: &Capabilities,
    resolved_capabilities: &ResolvedCapabilities<'_>,
) -> Result<(), Outcome> {
    let program = machine.program;
    let index = site.foreign.map_or(usize::MAX, |import| import as usize);
    let Some(import) = program.imports.get(index) else {
        return Err(Outcome::Trap("KLF-VM-0001".to_owned()));
    };
    let start = site.first_arg as usize;
    let arguments = &registers[start..start + site.arg_count as usize];
    let symbol = import.symbol.as_str();
    let value = match &import.kind {
        ImportKind::Address => resolved(machine, index)? as u64,
        ImportKind::Pointer(_) => {
            let address = site
                .target
                .and_then(|register| registers.get(register as usize).copied())
                .ok_or_else(|| Outcome::Trap("KLF-VM-0001".to_owned()))?;
            let plan = resolved_capabilities
                .native_plan(index)
                .ok_or_else(|| Outcome::Trap("KLF-VM-0001".to_owned()))?;
            native_call(
                machine,
                address as usize,
                plan,
                arguments,
                capabilities,
                resolved_capabilities,
            )?
        }
        ImportKind::Library { .. } => return Err(Outcome::Trap("KLF-VM-0001".to_owned())),
        ImportKind::Call(_) | ImportKind::Variadic { .. } => {
            if symbol == ending::EXIT {
                return Err(Outcome::Return(Some(
                    arguments.first().copied().unwrap_or(0),
                )));
            }
            if symbol == ending::AT_EXIT {
                ending::replace(&mut machine.exit_function, arguments)
            } else if let Some(value) =
                resolved_capabilities.call_with_memory(index, arguments, &mut machine.heap)
            {
                value
            } else {
                let address = resolved(machine, index)?;
                let plan = resolved_capabilities
                    .native_plan(index)
                    .ok_or_else(|| Outcome::Trap("KLF-VM-0001".to_owned()))?;
                native_call(
                    machine,
                    address,
                    plan,
                    arguments,
                    capabilities,
                    resolved_capabilities,
                )?
            }
        }
    };
    if site.has_result {
        registers[site.result as usize] = value;
    }
    Ok(())
}

/// The native address of import `index`, resolved on first use.
fn resolved(machine: &mut Machine, index: usize) -> Result<usize, Outcome> {
    if machine.resolved[index] == 0 {
        let symbol = &machine.program.imports[index].symbol;
        machine.resolved[index] =
            crate::foreign::resolve(&machine.libraries, symbol).ok_or_else(|| {
                eprintln!("unresolved native symbol `{symbol}`");
                Outcome::Trap("KLF-VM-0004".to_owned())
            })?;
    }
    Ok(machine.resolved[index])
}

#[cfg(not(target_arch = "wasm32"))]
fn native_call(
    machine: &mut Machine,
    address: usize,
    plan: &crate::foreign::CallPlan,
    arguments: &[u64],
    capabilities: &Capabilities,
    resolved_capabilities: &ResolvedCapabilities<'_>,
) -> Result<u64, Outcome> {
    reentry::call(machine, capabilities, resolved_capabilities, || {
        crate::foreign::call(address, plan, arguments)
    })
    .map_err(Outcome::Trap)
}

#[cfg(target_arch = "wasm32")]
fn native_call(
    _machine: &mut Machine,
    address: usize,
    plan: &crate::foreign::CallPlan,
    arguments: &[u64],
    _capabilities: &Capabilities,
    _resolved_capabilities: &ResolvedCapabilities<'_>,
) -> Result<u64, Outcome> {
    crate::foreign::call(address, plan, arguments).map_err(Outcome::Trap)
}

enum Flow {
    Next,
    Jump(usize),
    Done(Outcome),
}

#[cfg(test)]
mod evaluation_tests {
    use super::*;
    use crate::bytecode::{Instr, Opcode};
    use crate::program::{BcFunction, CallSite, ForeignSignature, Import};
    use klf_core::Name;

    #[test]
    fn evaluation_budget_bounds_execution_and_denies_foreign_calls() {
        let mut program = Program {
            functions: vec![BcFunction {
                name: Name::new("evaluate"),
                register_count: 2,
                param_count: 2,
                returns_value: false,
                code: vec![Instr::from_bx(Opcode::Jmp, 0, 0).encode()],
                call_sites: vec![],
                allocations: vec![],
                branches: vec![],
            }],
            consts: vec![],
            imports: vec![Import {
                symbol: "abort".to_owned(),
                kind: ImportKind::Call(ForeignSignature::default()),
            }],
            globals: vec![],
            initializers: vec![],
        };
        let mut budget = EvaluationBudget {
            steps_remaining: 7,
            memory_bytes: 1 << 20,
        };
        let mut machine = Machine::for_evaluation(&program, &budget).ok().unwrap();
        assert_eq!(
            machine.evaluate(0, &[], &mut budget),
            Outcome::Trap("KLF-VM-0022".to_owned())
        );
        assert_eq!(budget.steps_remaining, 0);
        drop(machine);

        program.functions[0].code = vec![Instr::from_bx(Opcode::Call, 0, 0).encode()];
        program.functions[0].call_sites.push(CallSite {
            callee: 0,
            first_arg: 0,
            arg_count: 0,
            result: 0,
            has_result: false,
            foreign: Some(0),
            target: None,
            fallible: None,
        });
        budget.steps_remaining = 7;
        let mut machine = Machine::for_evaluation(&program, &budget).ok().unwrap();
        assert_eq!(
            machine.evaluate(0, &[], &mut budget),
            Outcome::Trap("KLF-VM-0024".to_owned())
        );
        assert_eq!(budget.steps_remaining, 6);
        drop(machine);

        program.functions[0].code = vec![Instr::abc(Opcode::HallocR, 0, 0, 1).encode()];
        let mut machine = Machine::for_evaluation(&program, &budget).ok().unwrap();
        assert_eq!(
            machine.evaluate(0, &[u64::MAX, 8], &mut budget),
            Outcome::Trap("KLF-VM-0023".to_owned())
        );
        assert_eq!(budget.steps_remaining, 5);
    }

    #[test]
    fn evaluation_reads_a_foreign_address_as_zero() {
        let program = Program {
            functions: vec![BcFunction {
                name: Name::new("evaluate"),
                register_count: 2,
                param_count: 1,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::Call, 0, 0).encode(),
                    Instr::abc(Opcode::RetV, 1, 0, 0).encode(),
                ],
                call_sites: vec![CallSite {
                    callee: 0,
                    first_arg: 0,
                    arg_count: 0,
                    result: 1,
                    has_result: true,
                    foreign: Some(0),
                    target: None,
                    fallible: None,
                }],
                allocations: vec![],
                branches: vec![],
            }],
            consts: vec![],
            imports: vec![Import {
                symbol: "no_such_symbol".to_owned(),
                kind: ImportKind::Address,
            }],
            globals: vec![],
            initializers: vec![],
        };
        let mut budget = EvaluationBudget {
            steps_remaining: 7,
            memory_bytes: 1 << 20,
        };
        let mut machine = Machine::for_evaluation(&program, &budget).ok().unwrap();
        assert_eq!(
            machine.evaluate(0, &[7], &mut budget),
            Outcome::Return(Some(0))
        );
    }
}
