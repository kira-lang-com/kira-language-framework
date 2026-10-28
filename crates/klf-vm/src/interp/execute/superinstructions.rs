use super::super::EvaluationBudget;
use super::super::Outcome;
use super::super::decoded::{DecodedAddLoad, DecodedAllocaPair, DecodedInstr, DecodedMovePair};
use super::super::frames::{Frame, unwind};
use super::helpers::{read_register, write_register};
use super::memory::{execute_decoded_memory, execute_stack_allocation};
use klf_rt::Heap;

#[inline(always)]
pub(super) fn execute_move_pair<const METERED: bool>(
    pair: Option<DecodedMovePair>,
    registers: &mut [u64],
    pc: &mut usize,
) -> bool {
    let Some(pair) = pair else {
        return false;
    };
    let first = read_register(registers, pair.first_b);
    write_register(registers, pair.first_a, first);
    if METERED {
        *pc += 1;
        return true;
    }
    let second = read_register(registers, pair.second_b);
    write_register(registers, pair.second_a, second);
    *pc += 2;
    true
}

#[inline(always)]
pub(super) fn execute_alloca_pair<const METERED: bool>(
    pair: Option<DecodedAllocaPair>,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    budget: &mut Option<&mut EvaluationBudget>,
    pc: &mut usize,
) -> Option<Outcome> {
    let Some(pair) = pair else {
        return Some(unwind(
            active,
            callers,
            heap,
            Outcome::Trap("KLF-VM-0001".to_owned()),
        ));
    };
    if METERED {
        if budget.as_deref().is_some_and(|budget| {
            !heap.allocation_fits(
                pair.first_size as usize,
                pair.first_align.max(1) as usize,
                budget.memory_bytes,
            )
        }) {
            return Some(unwind(
                active,
                callers,
                heap,
                Outcome::Trap("KLF-VM-0023".to_owned()),
            ));
        }
        let first =
            execute_stack_allocation::<METERED>(pair.first_size, pair.first_align, active, heap);
        write_register(&mut active.registers, pair.first_a, first as u64);
        *pc += 1;
        return None;
    }
    let first =
        execute_stack_allocation::<METERED>(pair.first_size, pair.first_align, active, heap);
    write_register(&mut active.registers, pair.first_a, first as u64);
    let second =
        execute_stack_allocation::<METERED>(pair.second_size, pair.second_align, active, heap);
    write_register(&mut active.registers, pair.second_a, second as u64);
    *pc += 2;
    None
}

#[inline(always)]
pub(super) fn execute_add_load<const METERED: bool>(
    operation: Option<DecodedAddLoad>,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    pc: &mut usize,
) -> Option<Outcome> {
    let Some(operation) = operation else {
        return Some(unwind(
            active,
            callers,
            heap,
            Outcome::Trap("KLF-VM-0001".to_owned()),
        ));
    };
    let address = read_register(&active.registers, operation.add_b)
        .wrapping_add(read_register(&active.registers, operation.add_c));
    write_register(&mut active.registers, operation.add_a, address);
    if METERED {
        *pc += 1;
        return None;
    }
    let outcome = match operation.load_width {
        1 => execute_decoded_memory::<1, true>(
            operation.load_a,
            operation.add_a,
            active,
            callers,
            heap,
            pc,
        ),
        2 => execute_decoded_memory::<2, true>(
            operation.load_a,
            operation.add_a,
            active,
            callers,
            heap,
            pc,
        ),
        4 => execute_decoded_memory::<4, true>(
            operation.load_a,
            operation.add_a,
            active,
            callers,
            heap,
            pc,
        ),
        8 => execute_decoded_memory::<8, true>(
            operation.load_a,
            operation.add_a,
            active,
            callers,
            heap,
            pc,
        ),
        _ => {
            return Some(unwind(
                active,
                callers,
                heap,
                Outcome::Trap("KLF-VM-0001".to_owned()),
            ));
        }
    };
    if let Some(outcome) = outcome {
        return Some(outcome);
    }
    *pc += 1;
    None
}

#[inline(always)]
pub(super) fn execute_load_move<const BYTES: u32, const METERED: bool>(
    load_a: u16,
    load_b: u16,
    move_a: u16,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    pc: &mut usize,
) -> Option<Outcome> {
    if let Some(outcome) =
        execute_decoded_memory::<BYTES, true>(load_a, load_b, active, callers, heap, pc)
    {
        return Some(outcome);
    }
    if !METERED {
        let value = read_register(&active.registers, load_a);
        write_register(&mut active.registers, move_a, value);
        *pc += 1;
    }
    None
}

pub(super) fn execute_load_move_instruction<const METERED: bool>(
    instruction: DecodedInstr,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    pc: &mut usize,
) -> Option<Outcome> {
    let (width, load_a, load_b, move_a) = match instruction {
        DecodedInstr::Load8Move {
            load_a,
            load_b,
            move_a,
        } => (1, load_a, load_b, move_a),
        DecodedInstr::Load16Move {
            load_a,
            load_b,
            move_a,
        } => (2, load_a, load_b, move_a),
        DecodedInstr::Load32Move {
            load_a,
            load_b,
            move_a,
        } => (4, load_a, load_b, move_a),
        DecodedInstr::Load64Move {
            load_a,
            load_b,
            move_a,
        } => (8, load_a, load_b, move_a),
        _ => unreachable!("not a decoded load-move instruction"),
    };
    match width {
        1 => execute_load_move::<1, METERED>(load_a, load_b, move_a, active, callers, heap, pc),
        2 => execute_load_move::<2, METERED>(load_a, load_b, move_a, active, callers, heap, pc),
        4 => execute_load_move::<4, METERED>(load_a, load_b, move_a, active, callers, heap, pc),
        8 => execute_load_move::<8, METERED>(load_a, load_b, move_a, active, callers, heap, pc),
        _ => unreachable!("decoded load width"),
    }
}
