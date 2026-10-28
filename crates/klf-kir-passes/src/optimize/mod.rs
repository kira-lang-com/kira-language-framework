//! The optimization pipeline of section 14.1. Tier 1 is the same for every
//! backend; tier 2 reshapes its result for the backend that will compile it.

mod borrow;
mod cfg;
mod clean;
mod convert;
mod devirt;
mod fold;
mod graph;
mod hoist;
mod inline;
mod loops;
mod machine;
mod made;
mod memo;
mod memory;
mod motion;
mod pairing;
mod unique;
mod util;

use inline::Budget;
use klf_core::Name;
use klf_kir::{Function, Module, ModuleIndex, Operation, ValueTypes};
use klf_layout::Layouts;
use made::Made;
pub use memo::Memo;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    O0,
    O1,
    O2,
}

/// The backend tier 2 shapes the module for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Cranelift,
    Llvm,
    Vm,
}

pub(crate) struct Context<'a> {
    pub(crate) index: &'a ModuleIndex<'a>,
    pub(crate) layouts: &'a Layouts<'a>,
    /// The module without bodies: its traits and witness tables.
    pub(crate) module: &'a Module,
    /// Inside [`simplify`], the value types of its function until a pass
    /// changes it.
    types: RefCell<KeptTypes>,
}

/// What a [`Context`] keeps of the value types of the function it simplifies.
#[derive(Clone)]
enum KeptTypes {
    /// Outside [`simplify`]: nothing is kept.
    Off,
    /// Inside it, before they are inferred or since a pass changed the
    /// function.
    Stale,
    Known(Rc<ValueTypes>),
}

impl Context<'_> {
    /// The type of every value of `function`.
    pub(crate) fn types(&self, function: &Function) -> Rc<ValueTypes> {
        let mut kept = self.types.borrow_mut();
        match &*kept {
            KeptTypes::Known(types) => Rc::clone(types),
            KeptTypes::Stale => {
                let types = Rc::new(klf_kir::infer_value_types_in(function, self.index));
                *kept = KeptTypes::Known(Rc::clone(&types));
                types
            }
            KeptTypes::Off => Rc::new(klf_kir::infer_value_types_in(function, self.index)),
        }
    }

    /// Forgets the types kept of the function a pass is adding values to.
    pub(crate) fn forget_types(&self) {
        let mut kept = self.types.borrow_mut();
        if !matches!(*kept, KeptTypes::Off) {
            *kept = KeptTypes::Stale;
        }
    }

    /// Answers whether the pass `pass` changed `function`, forgetting the
    /// types kept of it when it did.
    fn changed(&self, changed: bool, pass: &str, function: &Function) -> bool {
        let mut kept = self.types.borrow_mut();
        if changed && !matches!(*kept, KeptTypes::Off) {
            *kept = KeptTypes::Stale;
        } else if cfg!(debug_assertions)
            && let KeptTypes::Known(types) = &*kept
        {
            assert!(
                **types == klf_kir::infer_value_types_in(function, self.index),
                "{pass} changed {} and said it did not",
                function.name
            );
        }
        changed
    }
}

const TRIVIAL: Budget = Budget {
    callee: 12,
    marked: 400,
    caller: 20_000,
};

const COST_MODEL: Budget = Budget {
    callee: 60,
    marked: 400,
    caller: 20_000,
};

const CRANELIFT: Budget = Budget {
    callee: 160,
    marked: 400,
    caller: 30_000,
};

/// Runs tier 1 at `level`, then tier 2 for `backend`. O0 leaves the module
/// as the mandatory passes made it.
pub fn optimize(module: &mut Module, level: Level, backend: Backend) {
    optimize_with(module, level, backend, None);
}

/// [`optimize`], taking each function an earlier build optimized the same
/// way from `memo` and keeping what this one optimizes there.
pub fn optimize_with(module: &mut Module, level: Level, backend: Backend, memo: Option<&dyn Memo>) {
    if level == Level::O0 {
        return;
    }
    let promote = level == Level::O2;
    if promote {
        borrow::borrow_arguments(module);
    }
    let budget = if promote { COST_MODEL } else { TRIVIAL };
    let slow = inline::slow_paths(module, CRANELIFT.callee, TRIVIAL.callee);
    // Cranelift reads nothing of where an instruction was written.
    let placeless = backend == Backend::Cranelift;
    let first = Run {
        budget: Some(budget),
        promote,
        shape: false,
        placeless,
        name: "first",
    };
    let made = run(module, &first, Vec::new(), memo, &slow);
    let made = match backend {
        Backend::Cranelift => {
            let second = Run {
                budget: Some(CRANELIFT),
                promote,
                shape: true,
                placeless,
                name: "cranelift",
            };
            run(module, &second, made, memo, &slow);
            return;
        }
        Backend::Llvm => made,
        Backend::Vm => {
            let second = Run {
                budget: None,
                promote,
                shape: false,
                placeless,
                name: "vm",
            };
            run(module, &second, made, memo, &slow)
        }
    };
    for (position, body) in made::bodies(made, memo).into_iter().enumerate() {
        if let Some(body) = body {
            module.functions[position] = body;
        }
    }
}

/// Inlines bottom-up over the call graph, simplifying each function before
/// its callers inline it. A function inlines only what lies in an earlier
/// wave of the graph, so what it becomes depends on the graph alone; it is
/// optimized as soon as a worker is free and waits only for the callees it
/// inlines, never for a whole wave. With `shape`, each function is then
/// shaped for a backend that reads machine flags and leaves calls to its
/// register allocator, once it is optimized, and the module takes the
/// shaped bodies. Otherwise it is left as it was and what the run made of
/// each function is answered, for the next run to start from; `before` is
/// that of the last run. No pass changes a signature, so the module's
/// functions serve as the shell of every run.
#[allow(clippy::too_many_lines)]
fn run(
    module: &mut Module,
    this: &Run,
    before: Vec<Option<Made>>,
    memo: Option<&dyn Memo>,
    slow: &HashSet<Name>,
) -> Vec<Option<Made>> {
    let Run {
        budget,
        promote,
        shape,
        placeless,
        ..
    } = *this;
    let functions = std::mem::take(&mut module.functions);
    let count = functions.len();
    let before: Vec<Option<Made>> = if before.is_empty() {
        (0..count).map(|_| None).collect()
    } else {
        before
    };
    let has_body =
        |position: usize| before[position].is_some() || !functions[position].blocks.is_empty();
    let input = |position: usize| -> &Function {
        before[position]
            .as_ref()
            .map_or(&functions[position], |made| made.body(memo))
    };
    let shell = signatures(module, &functions);
    let index = ModuleIndex::of(&shell);
    let positions: HashMap<Name, usize> = functions
        .iter()
        .enumerate()
        .map(|(position, function)| (function.name.clone(), position))
        .collect();
    let scanned: Vec<Vec<Name>> = (0..count)
        .map(|position| match before[position] {
            Some(_) => Vec::new(),
            None => graph::calls_of(&functions[position]),
        })
        .collect();
    let calls: Vec<&[Name]> = (0..count)
        .map(|position| {
            before[position]
                .as_ref()
                .map_or(&scanned[position][..], |made| &made.calls[..])
        })
        .collect();
    let graph = graph::bottom_up(&calls, &positions);
    let run_digest = memo.map(|memo| memo::shell_digest(memo, &shell, &this.describe()));
    let mut wave_of = vec![usize::MAX; count];
    for (wave, members) in graph.waves.iter().enumerate() {
        for &position in members {
            wave_of[position] = wave;
        }
    }
    // Waves in order, so whatever a function waits for was handed out first.
    // Shaping a function, the task `count + position`, comes after every
    // function is handed out to be optimized.
    let mut order: Vec<usize> = graph
        .waves
        .iter()
        .flatten()
        .copied()
        .filter(|&position| has_body(position))
        .collect();
    if shape {
        order.extend((0..count).map(|position| count + position));
    }
    let shaped: Vec<OnceLock<Function>> = (0..count).map(|_| OnceLock::new()).collect();
    let done: Vec<OnceLock<Made>> = (0..count).map(|_| OnceLock::new()).collect();
    let finished = Finished::default();
    let settled = |position: usize| before[position].as_ref().is_some_and(|made| made.at_rest);
    // What `position` is offered of the function `name` to inline.
    let offer = |position: usize, name: &Name| -> Option<(&Made, bool)> {
        if slow.contains(name) {
            return None;
        }
        let &callee = positions.get(name)?;
        if !has_body(callee) || wave_of[callee] >= wave_of[position] {
            return None;
        }
        let made = finished.wait(&done[callee]);
        let cycle = graph.cycle[callee];
        ((cycle.is_none() || cycle != graph.cycle[position]) && made.inlinable)
            .then_some((made, cycle.is_some()))
    };
    let optimize_one = |position: usize,
                        context: &Context<'_>,
                        offered: &RefCell<Vec<(Name, memo::Offer)>>|
     -> (Function, bool) {
        let mut function = input(position).clone();
        util::start_fresh(&function);
        // A settled function simplifies to itself, so only what this run
        // changes in it needs simplifying again.
        let mut simplified = settled(position);
        let mut unchanged = settled(position);
        if let Some(mut budget) = budget {
            // A function small enough to inline stays so: what it inlines
            // must leave it within the budget its callers inline by.
            if !simplified {
                simplified = simplify(&mut function, context, promote);
            }
            if graph.called.contains(&position) && inline::size(&function) <= budget.callee {
                budget.caller = budget.callee;
            }
            // A function on a cycle inlines into callers off the cycle only.
            let bodies = |name: &Name| -> Option<(&Function, bool)> {
                let found = offer(position, name);
                let mut offered = offered.borrow_mut();
                if !offered.iter().any(|(seen, _)| seen == name) {
                    offered.push((
                        name.clone(),
                        found.map(|(made, cycle)| (made.digest, cycle)),
                    ));
                }
                found.map(|(made, cycle)| (made.body(memo), cycle))
            };
            if inline::inline_calls(&mut function, &bodies, budget) {
                simplified = false;
                unchanged = false;
            }
        }
        if !simplified {
            simplified = simplify(&mut function, context, promote);
        }
        // Versioning the function the last run versioned to the end finds
        // what it found then: nothing.
        let versioned = promote
            && !unchanged
            && loops::version(&mut function, context, &|trial| {
                simplify(trial, context, true);
            });
        (function, simplified && !versioned)
    };
    let digest_of = |memo: &dyn Memo, position: usize| {
        before[position].as_ref().map_or_else(
            || memo::digest_of(memo, &functions[position], placeless),
            |made| made.digest,
        )
    };
    on_workers(&order, &shell, &index, &finished, |task, context| {
        if task < count {
            let position = task;
            let Some((memo, run_digest)) = memo.zip(run_digest) else {
                let (function, at_rest) = optimize_one(position, context, &RefCell::default());
                finished.set(&done[position], Made::here(function, at_rest, [0; 32]));
                return;
            };
            let body = digest_of(memo, position);
            let facts = [settled(position), graph.called.contains(&position)];
            let key = memo::key(memo, &run_digest, &body, &facts);
            // An entry stands while every function it was offered is offered
            // alike now.
            let kept = memo::entry::<memo::Optimized>(memo, &key).filter(|entry| {
                entry.offered.iter().all(|(name, was)| {
                    offer(position, name).map(|(made, cycle)| (made.digest, cycle)) == *was
                })
            });
            let made = if let Some(entry) = kept {
                memo.reuse(&entry.body);
                memo.reuse(&key);
                Made::kept(entry)
            } else {
                let offered = RefCell::default();
                let (function, at_rest) = optimize_one(position, context, &offered);
                let body = memo::keep_body(memo, &function, placeless);
                let made = Made::here(function, at_rest, body);
                memo::keep(memo, key, &made.entry(offered.into_inner()));
                made
            };
            finished.set(&done[position], made);
            return;
        }
        let position = task - count;
        let made = has_body(position).then(|| finished.wait(&done[position]));
        let key = memo.zip(run_digest).map(|(memo, run_digest)| {
            let body = made.map_or_else(
                || memo::digest_of(memo, &functions[position], placeless),
                |made| made.digest,
            );
            (
                memo,
                memo::key(memo, &run_digest, &body, &[true, true, true]),
            )
        });
        if let Some((memo, key)) = key
            && let Some(digest) = memo::entry::<[u8; 32]>(memo, &key)
            && let Some(function) = memo::body(memo, &digest)
        {
            memo.reuse(&digest);
            memo.reuse(&key);
            let _ = shaped[position].set(function);
            return;
        }
        let mut function = made
            .map_or(&functions[position], |made| made.body(memo))
            .clone();
        util::start_fresh(&function);
        machine::shape(&mut function, context);
        if let Some((memo, key)) = key {
            let digest = memo::keep_body(memo, &function, placeless);
            memo::keep(memo, key, &digest);
        }
        let _ = shaped[position].set(function);
    });
    drop(before);
    let made: Vec<Option<Made>> = done.into_iter().map(OnceLock::into_inner).collect();
    if shape {
        module.functions = shaped
            .into_iter()
            .map(|function| function.into_inner().expect("every function is shaped"))
            .collect();
    } else {
        module.functions = functions;
    }
    made
}

/// One run of the optimizer over every function.
struct Run {
    budget: Option<Budget>,
    promote: bool,
    /// Whether each function is then shaped for a machine backend.
    shape: bool,
    /// Whether a memo names functions by what they are alone, not where
    /// they were written.
    placeless: bool,
    name: &'static str,
}

impl Run {
    /// Everything about the run its result depends on, for a memo.
    fn describe(&self) -> String {
        let budget = self
            .budget
            .map(|budget| (budget.callee, budget.marked, budget.caller));
        format!(
            "{} {budget:?} promote={} shape={} placeless={} {}",
            self.name,
            self.promote,
            self.shape,
            self.placeless,
            env!("CARGO_PKG_VERSION")
        )
    }
}

/// Tells the workers waiting on a function's result that it is there, or
/// that a worker failed and none will come.
#[derive(Default)]
struct Finished {
    lock: Mutex<()>,
    changed: Condvar,
    failed: AtomicBool,
}

impl Finished {
    fn set<T>(&self, cell: &OnceLock<T>, value: T) {
        {
            let _held = self
                .lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = cell.set(value);
        }
        self.changed.notify_all();
    }

    fn fail(&self) {
        {
            let _held = self
                .lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.failed.store(true, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }

    fn wait<'c, T>(&self, cell: &'c OnceLock<T>) -> &'c T {
        if let Some(value) = cell.get() {
            return value;
        }
        let mut held = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(value) = cell.get() {
                return value;
            }
            assert!(
                !self.failed.load(Ordering::Relaxed),
                "a function the optimizer waited for failed"
            );
            held = self
                .changed
                .wait(held)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// `work` for every one of `order`, handed out in that order to one worker
/// per core, each with a context of its own.
fn on_workers(
    order: &[usize],
    shell: &Module,
    index: &ModuleIndex<'_>,
    finished: &Finished,
    work: impl Fn(usize, &Context<'_>) + Sync,
) {
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(order.len());
    let next = AtomicUsize::new(0);
    let worker = || {
        let layouts = Layouts::new(&shell.types);
        let context = Context {
            index,
            layouts: &layouts,
            module: shell,
            types: RefCell::new(KeptTypes::Off),
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(&position) = order.get(at) else {
                    break;
                };
                work(position, &context);
            }
        }));
        if let Err(panic) = outcome {
            finished.fail();
            std::panic::resume_unwind(panic);
        }
    };
    if workers <= 1 {
        worker();
        return;
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(worker)).collect();
        for handle in handles {
            if let Err(panic) = handle.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}

/// Answers whether the function came to rest: a last round of every pass
/// changed nothing.
fn simplify(function: &mut Function, context: &Context<'_>, promote: bool) -> bool {
    let outer = context.types.replace(KeptTypes::Stale);
    let at_rest = simplify_rounds(function, context, promote);
    context.types.replace(outer);
    at_rest
}

fn simplify_rounds(function: &mut Function, context: &Context<'_>, promote: bool) -> bool {
    strip_debug(function);
    cfg::remove_unreachable(function);
    macro_rules! pass {
        ($pass:ident :: $($path:ident)::+ ($($argument:expr),*)) => {{
            let changed = $pass::$($path)::+($($argument),*);
            context.changed(changed, stringify!($pass $(::$path)+), function)
        }};
    }
    for _ in 0..4 {
        let mut changed = false;
        if promote {
            changed |= pass!(memory::split_copies(function, context));
            changed |= pass!(memory::split_slots(function, context));
            changed |= pass!(memory::promote::promote(function));
            changed |= pass!(memory::forward::forward(function, context));
            changed |= pass!(devirt::devirtualize(function, context));
            changed |= pass!(hoist::hoist(function));
            changed |= pass!(unique::elide(function));
            changed |= pass!(motion::tighten(function, context));
            changed |= pass!(convert::convert(function, context));
        }
        changed |= pass!(pairing::pair(function, context));
        changed |= pass!(fold::fold(function, context));
        changed |= pass!(clean::remove_dead(function, context));
        changed |= pass!(clean::remove_parameters(function));
        changed |= pass!(cfg::simplify(function));
        if !changed {
            return true;
        }
    }
    false
}

/// Debug records keep values and slots alive that an optimized build has
/// no use for.
fn strip_debug(function: &mut Function) {
    for block in &mut function.blocks {
        block.instructions.retain(|instruction| {
            !matches!(
                instruction.operation,
                Operation::DebugValue { .. } | Operation::DebugAddr { .. }
            )
        });
    }
}

/// The module without function bodies: what passes need to type a call's
/// results while the bodies themselves change.
fn signatures(module: &Module, functions: &[Function]) -> Module {
    let mut shell = module.clone();
    shell.functions = functions
        .iter()
        .map(|function| Function {
            blocks: Vec::new(),
            ..function.clone()
        })
        .collect();
    shell
}
