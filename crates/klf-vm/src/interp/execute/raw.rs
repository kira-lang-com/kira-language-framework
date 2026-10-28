//! The instructions the decoder leaves in their raw form, run one at a time:
//! those `step` runs, plus `setjmp` and `longjmp` (section 12.14), which
//! need the frames.
//!
//! `setjmp` writes five words into its buffer: a mark, then how many frames
//! are below its own, its frame's depth and function, and where to resume
//! with which register to answer in. `longjmp` leaves the frames above that
//! one and resumes it there, answering its value, or 1 for 0. A buffer
//! without the mark, or one whose frame has returned, is a fault.

use super::super::frames::{
    Frame, FramePool, catch_failure, double_free, finish_return, leave_to, unwind,
};
use super::super::step::step;
use super::super::{Flow, Outcome};
use crate::bytecode::{Instr, Opcode};
use crate::program::Program;
use klf_rt::Heap;

const MARK: u64 = 0x4b4c_465f_4a4d_5042;
const FAULT: &str = "KLF-VM-0001";

pub(super) enum Raw {
    Next,
    Jump(usize),
    /// The active frame changed; reload its code.
    Switched,
    /// A `longjmp` resumed a `setjmp`, which may close a loop.
    LongJumped,
    End(Outcome),
}

pub(super) fn execute_raw(
    program: &Program,
    instruction: Instr,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    pool: &mut FramePool,
    heap: &mut Heap,
    pc: &mut usize,
) -> Raw {
    match instruction.op {
        Opcode::SetJmp => return set_jump(instruction, active, callers.len(), heap, *pc),
        Opcode::LongJmp => return long_jump(instruction, active, callers, pool, heap, pc),
        _ => {}
    }
    match step(
        program,
        instruction,
        &mut active.registers[..active.register_count],
    ) {
        Flow::Next => Raw::Next,
        Flow::Jump(target) => Raw::Jump(target),
        Flow::Done(Outcome::Return(value)) => {
            match finish_return(active, callers, pool, heap, pc, value) {
                Some(outcome) => Raw::End(outcome),
                None => Raw::Switched,
            }
        }
        Flow::Done(Outcome::Fail(value)) => {
            match catch_failure(active, callers, pool, heap, pc, value) {
                Some(outcome) => Raw::End(outcome),
                None => Raw::Switched,
            }
        }
        Flow::Done(other) => Raw::End(unwind(active, callers, heap, other)),
    }
}

fn set_jump(
    instruction: Instr,
    active: &mut Frame,
    below: usize,
    heap: &mut Heap,
    pc: usize,
) -> Raw {
    let buffer = active.registers[usize::from(instruction.b)] as *mut u8;
    let resume = (pc as u64 + 1) | (u64::from(instruction.a) << 48);
    let words = [
        MARK,
        below as u64,
        active.depth as u64,
        active.function as u64,
        resume,
    ];
    for (index, word) in words.into_iter().enumerate() {
        if !heap.write(buffer.wrapping_add(index * 8), 8, word) {
            return Raw::End(Outcome::Trap("KLF-RT-0005".to_owned()));
        }
    }
    active.registers[usize::from(instruction.a)] = 0;
    Raw::Next
}

fn long_jump(
    instruction: Instr,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    pool: &mut FramePool,
    heap: &mut Heap,
    pc: &mut usize,
) -> Raw {
    let buffer = active.registers[usize::from(instruction.a)] as *const u8;
    let value = active.registers[usize::from(instruction.b)] as u32;
    let mut words = [0u64; 5];
    for (index, word) in words.iter_mut().enumerate() {
        match heap.read(buffer.wrapping_add(index * 8), 8) {
            Some(read) => *word = read,
            None => return Raw::End(Outcome::Trap("KLF-RT-0005".to_owned())),
        }
    }
    let [mark, below, depth, function, resume] = words;
    if mark != MARK || below as usize > callers.len() {
        return Raw::End(unwind(
            active,
            callers,
            heap,
            Outcome::Trap(FAULT.to_owned()),
        ));
    }
    if !leave_to(active, callers, pool, heap, below as usize) {
        return Raw::End(unwind(active, callers, heap, double_free()));
    }
    let register = (resume >> 48) as usize;
    if active.depth as u64 != depth
        || active.function as u64 != function
        || register >= active.register_count
    {
        return Raw::End(unwind(
            active,
            callers,
            heap,
            Outcome::Trap(FAULT.to_owned()),
        ));
    }
    let answer = if value == 0 { 1 } else { value as i32 };
    active.registers[register] = i64::from(answer) as u64;
    *pc = (resume & 0xffff_ffff_ffff) as usize;
    Raw::LongJumped
}
