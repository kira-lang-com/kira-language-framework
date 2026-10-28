use super::super::Outcome;
use super::super::frames::{Frame, unwind};
use super::helpers::{read_register, write_register};
use klf_rt::Heap;

#[inline(always)]
pub(super) fn execute_decoded_memory<const BYTES: u32, const IS_LOAD: bool>(
    a: u16,
    b: u16,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    pc: &mut usize,
) -> Option<Outcome> {
    let okay = if IS_LOAD {
        let pointer = read_register(&active.registers, b) as *const u8;
        if let Some(value) = heap.read(pointer, BYTES) {
            write_register(&mut active.registers, a, value);
            true
        } else {
            false
        }
    } else {
        let pointer = read_register(&active.registers, a) as *mut u8;
        heap.write(pointer, BYTES, read_register(&active.registers, b))
    };
    if !okay {
        return Some(unwind(
            active,
            callers,
            heap,
            Outcome::Trap("KLF-RT-0005".to_owned()),
        ));
    }
    *pc += 1;
    None
}

#[inline(always)]
pub(super) fn execute_stack_allocation<const METERED: bool>(
    size: u32,
    align: u32,
    active: &mut Frame,
    heap: &mut Heap,
) -> *mut u8 {
    if !METERED && let Some(pointer) = heap.arena_alloc_raw(size as usize, align.max(1) as usize) {
        return pointer;
    }
    let pointer = heap.alloc_raw(size as usize, align.max(1) as usize);
    active.stack.push(pointer);
    pointer
}

#[inline(always)]
pub(super) fn execute_allocation<const METERED: bool>(
    register: u16,
    shape: crate::program::AllocShape,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    budget: &mut Option<&mut super::super::EvaluationBudget>,
    pc: &mut usize,
) -> Option<Outcome> {
    if METERED
        && budget.as_deref().is_some_and(|budget| {
            !heap.allocation_fits(
                shape.size as usize,
                shape.align.max(1) as usize,
                budget.memory_bytes,
            )
        })
    {
        return Some(unwind(
            active,
            callers,
            heap,
            Outcome::Trap("KLF-VM-0023".to_owned()),
        ));
    }
    let pointer = execute_stack_allocation::<METERED>(shape.size, shape.align, active, heap);
    write_register(&mut active.registers, register, pointer as u64);
    *pc += 1;
    None
}

#[inline(always)]
pub(super) fn execute_alloca_group<const METERED: bool>(
    group: &super::super::decoded::DecodedAllocaGroup,
    active: &mut Frame,
    callers: &mut Vec<Frame>,
    heap: &mut Heap,
    budget: &mut Option<&mut super::super::EvaluationBudget>,
    pc: &mut usize,
) -> Option<Outcome> {
    if METERED {
        let first = &group.slots[0];
        return execute_allocation::<METERED>(
            first.register,
            first.shape,
            active,
            callers,
            heap,
            budget,
            pc,
        );
    }
    if let Some(pointer) = heap.arena_alloc_raw(group.size as usize, group.align as usize) {
        for slot in &group.slots {
            write_register(
                &mut active.registers,
                slot.register,
                pointer.wrapping_add(slot.offset as usize) as u64,
            );
        }
        *pc += group.slots.len();
    } else {
        for slot in &group.slots {
            execute_allocation::<METERED>(
                slot.register,
                slot.shape,
                active,
                callers,
                heap,
                budget,
                pc,
            );
        }
    }
    None
}
