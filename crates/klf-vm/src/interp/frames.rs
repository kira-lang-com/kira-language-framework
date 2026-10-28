use super::Outcome;
use super::access::Accesses;
use super::decoded::DecodedFunction;
use crate::program::{CallSite, Program};
use klf_rt::Heap;

pub(super) struct Frame {
    /// Which version of the module's code this frame runs (section 17.5).
    /// A pushed version is installed only between entry calls, so every frame
    /// of one call shares the version its entry took.
    pub(super) version: u8,
    pub(super) function: usize,
    pub(super) registers: Vec<u64>,
    pub(super) register_count: usize,
    pub(super) pc: usize,
    pub(super) stack: Vec<*mut u8>,
    result: u16,
    has_result: bool,
    /// The caller's register a `try.call` reads to pick its edge: 1 when this
    /// frame fails, 0 when it returns. `None` lets a failure propagate.
    failed: Option<u16>,
    pub(super) branch_scratch: Vec<u64>,
    pub(super) depth: usize,
    arena_mark: (usize, usize),
    /// The dynamic exclusivity claims this frame holds (section 11.8).
    pub(super) accesses: Accesses,
}

pub(super) type FramePool = Vec<Frame>;

pub(super) fn restore_pool(target: &mut FramePool, mut pool: FramePool, active: Frame) {
    pool.push(active);
    pool.append(target);
    *target = pool;
}

pub(super) fn can_tail_call(frame: &Frame, heap: &Heap) -> bool {
    frame.stack.is_empty() && heap.arena_mark() == frame.arena_mark
}

#[allow(clippy::too_many_arguments)]
pub(super) fn make_frame(
    program: &Program,
    decoded: &[DecodedFunction],
    version: u8,
    function: usize,
    arguments: &[u64],
    result: u16,
    has_result: bool,
    failed: Option<u16>,
    depth: usize,
    reusable: Option<Frame>,
    heap: &Heap,
) -> Frame {
    let mut frame = reusable.unwrap_or(Frame {
        version,
        function,
        registers: Vec::new(),
        register_count: 0,
        pc: 0,
        stack: Vec::new(),
        result,
        has_result,
        failed,
        branch_scratch: Vec::new(),
        depth,
        arena_mark: heap.arena_mark(),
        accesses: Accesses::default(),
    });
    frame.version = version;
    frame.function = function;
    let register_count = program.functions[function].register_count as usize;
    if frame.registers.len() < register_count {
        frame.registers.resize(register_count, 0);
    }
    frame.register_count = register_count;
    let copied = register_count.min(arguments.len());
    frame.registers[..copied].copy_from_slice(&arguments[..copied]);
    // KIR verification gives every local a definition; only missing entry args need reset.
    let parameter_end = (program.functions[function].param_count as usize).min(register_count);
    if arguments.len() < parameter_end {
        frame.registers[arguments.len()..parameter_end].fill(0);
    }
    frame.pc = 0;
    frame.stack.clear();
    // A frame entering claims nothing; whatever its predecessor held is gone.
    frame.accesses = Accesses::default();
    frame.result = result;
    frame.has_result = has_result;
    frame.failed = failed;
    frame.depth = depth;
    frame.arena_mark = heap.arena_mark();
    let branch_scratch = decoded[function].max_branch_moves;
    if frame.branch_scratch.len() < branch_scratch {
        frame.branch_scratch.resize(branch_scratch, 0);
    }
    frame
}

pub(super) fn make_entry_frame(
    program: &Program,
    decoded: &[DecodedFunction],
    version: u8,
    function: usize,
    arguments: &[u64],
    reusable: Option<Frame>,
    heap: &Heap,
) -> Frame {
    make_frame(
        program, decoded, version, function, arguments, 0, false, None, 1, reusable, heap,
    )
}

/// Frees the frame's stack slots; `false` when the heap found one already
/// free, which the caller reports as `KLF-RT-0006`.
fn free_stack(frame: &mut Frame, heap: &mut Heap) -> bool {
    let mut freed = true;
    for slot in frame.stack.drain(..) {
        freed &= heap.free_raw(slot);
    }
    freed
}

/// The outcome of freeing a slot that was already free.
pub(super) fn double_free() -> Outcome {
    Outcome::Trap("KLF-RT-0006".to_owned())
}

pub(super) fn finish_return(
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    pool: &mut FramePool,
    heap: &mut Heap,
    pc: &mut usize,
    value: Option<u64>,
) -> Option<Outcome> {
    let has_result = active.has_result;
    let result = active.result as usize;
    let failed = active.failed;
    if !free_stack(active, heap) {
        return Some(double_free());
    }
    heap.arena_release(active.arena_mark);
    if let Some(mut caller) = callers.pop() {
        if has_result {
            caller.registers[result] = value.unwrap_or(0);
        }
        if let Some(failed) = failed {
            caller.registers[failed as usize] = 0;
        }
        *pc = caller.pc;
        pool.push(std::mem::replace(active, caller));
        None
    } else {
        Some(Outcome::Return(value))
    }
}

/// A `fail` in `active`: frames give way until one was entered by a
/// `try.call`, whose caller then resumes after the call with the error in the
/// result register and the failed flag set. With no such frame the failure
/// ends the run.
pub(super) fn catch_failure(
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    pool: &mut FramePool,
    heap: &mut Heap,
    pc: &mut usize,
    value: u64,
) -> Option<Outcome> {
    loop {
        let failed = active.failed;
        let result = active.result as usize;
        let Some(mut caller) = callers.pop() else {
            return Some(unwind(active, callers, heap, Outcome::Fail(value)));
        };
        if !free_stack(active, heap) {
            return Some(double_free());
        }
        heap.arena_release(active.arena_mark);
        if let Some(failed) = failed {
            caller.registers[result] = value;
            caller.registers[failed as usize] = 1;
            *pc = caller.pc;
            pool.push(std::mem::replace(active, caller));
            return None;
        }
        pool.push(std::mem::replace(active, caller));
    }
}

/// Frames give way until `callers` holds `depth` of them, as a `longjmp`
/// leaves them: their slots are freed and nothing else runs. `false` when a
/// slot was already free.
pub(super) fn leave_to(
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    pool: &mut FramePool,
    heap: &mut Heap,
    depth: usize,
) -> bool {
    let mut freed = true;
    while callers.len() > depth {
        let Some(caller) = callers.pop() else { break };
        freed &= free_stack(active, heap);
        heap.arena_release(active.arena_mark);
        pool.push(std::mem::replace(active, caller));
    }
    freed
}

pub(super) fn unwind(
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    outcome: Outcome,
) -> Outcome {
    let mut freed = free_stack(active, heap);
    heap.arena_release(active.arena_mark);
    for mut frame in callers.drain(..).rev() {
        freed &= free_stack(&mut frame, heap);
        heap.arena_release(frame.arena_mark);
    }
    if freed { outcome } else { double_free() }
}

pub(super) fn callee_frame(
    program: &Program,
    decoded: &[DecodedFunction],
    site: CallSite,
    caller: &Frame,
    pool: &mut FramePool,
    heap: &Heap,
) -> Option<Frame> {
    let registers = &caller.registers[..caller.register_count];
    let target = match site.target {
        Some(register) => registers[register as usize] as usize,
        None => site.callee,
    };
    if target >= program.functions.len() {
        return None;
    }
    let start = site.first_arg as usize;
    let arguments = &registers[start..start + site.arg_count as usize];
    Some(make_frame(
        program,
        decoded,
        caller.version,
        target,
        arguments,
        site.result,
        site.has_result,
        site.fallible.map(|fallible| fallible.failed),
        caller.depth + 1,
        pool.pop(),
        heap,
    ))
}

pub(super) fn tail_call_frame(
    program: &Program,
    decoded: &[DecodedFunction],
    site: CallSite,
    frame: &mut Frame,
) -> Option<usize> {
    debug_assert_eq!(frame.stack, [] as [*mut u8; 0]);
    let callee = match site.target {
        Some(register) => *frame.registers[..frame.register_count].get(register as usize)? as usize,
        None => site.callee,
    };
    let function = program.functions.get(callee)?;
    let start = site.first_arg as usize;
    let end = start.checked_add(site.arg_count as usize)?;
    if end > frame.register_count {
        return None;
    }
    let register_count = function.register_count as usize;
    let copied = (end - start).min(register_count);
    frame.registers.copy_within(start..start + copied, 0);
    if frame.registers.len() < register_count {
        frame.registers.resize(register_count, 0);
    }
    frame.register_count = register_count;
    let parameter_end = (function.param_count as usize).min(register_count);
    if copied < parameter_end {
        frame.registers[copied..parameter_end].fill(0);
    }
    // A tail call stays in the version its caller entered.
    frame.function = callee;
    frame.depth += 1;
    let branch_scratch = decoded[callee].max_branch_moves;
    if frame.branch_scratch.len() < branch_scratch {
        frame.branch_scratch.resize(branch_scratch, 0);
    }
    Some(callee)
}
