use super::super::EvaluationBudget;
use super::super::decoded::{DecodedBranchSite, DecodedBranchTarget, DecodedFunction, join_u32};
use super::super::frames::Frame;
use crate::bytecode::{Instr, Opcode};
use crate::program::BcFunction;
use klf_rt::Heap;

#[inline(always)]
pub(super) fn read_register(registers: &[u64], index: u16) -> u64 {
    debug_assert!(usize::from(index) < registers.len());
    unsafe { *registers.get_unchecked(usize::from(index)) }
}

#[inline(always)]
pub(super) fn write_register(registers: &mut [u64], index: u16, value: u64) {
    debug_assert!(usize::from(index) < registers.len());
    unsafe { *registers.get_unchecked_mut(usize::from(index)) = value };
}

#[inline(always)]
pub(super) fn branch_at(function: &DecodedFunction, site: u32) -> &DecodedBranchSite {
    unsafe {
        function
            .branches
            .get_unchecked(site as usize)
            .as_ref()
            .unwrap_unchecked()
    }
}

#[inline(always)]
pub(super) fn join_i32(low: u16, high: u16) -> i32 {
    join_u32(low, high) as i32
}

#[inline(always)]
pub(super) fn enum_switch_equal(lhs: u64, rhs: u64, width: crate::bytecode::WidthCode) -> bool {
    match width {
        crate::bytecode::WidthCode::W8 => lhs as u8 == rhs as u8,
        crate::bytecode::WidthCode::W16 => lhs as u16 == rhs as u16,
        crate::bytecode::WidthCode::W32 => lhs as u32 == rhs as u32,
        crate::bytecode::WidthCode::W64 => lhs == rhs,
        crate::bytecode::WidthCode::W128 => false,
    }
}

pub(super) fn apply_branch_edge(frame: &mut Frame, edge: &DecodedBranchTarget) {
    if edge.needs_scratch {
        let scratch = &mut frame.branch_scratch;
        let registers = &frame.registers;
        for (slot, movement) in scratch.iter_mut().zip(&edge.moves) {
            *slot = read_register(registers, movement.source);
        }
        let scratch = &frame.branch_scratch;
        let registers = &mut frame.registers;
        for (slot, movement) in scratch.iter().zip(&edge.moves) {
            write_register(registers, movement.destination, *slot);
        }
    } else {
        for movement in &edge.moves {
            let value = read_register(&frame.registers, movement.source);
            write_register(&mut frame.registers, movement.destination, value);
        }
    }
}

#[inline(always)]
pub(super) fn spend_branch_moves<const METERED: bool>(
    heap: &Heap,
    budget: &mut Option<&mut EvaluationBudget>,
    count: usize,
) -> Option<&'static str> {
    if METERED {
        let budget = budget
            .as_deref_mut()
            .expect("metered execution has a budget");
        for _ in 0..count {
            if let Some(code) = spend_step(heap, budget) {
                return Some(code);
            }
        }
    }
    None
}

pub(super) fn spend_step(heap: &Heap, budget: &mut EvaluationBudget) -> Option<&'static str> {
    if heap.backing_bytes() > budget.memory_bytes {
        return Some("KLF-VM-0023");
    }
    if budget.steps_remaining == 0 {
        return Some("KLF-VM-0022");
    }
    budget.steps_remaining -= 1;
    None
}

pub(super) fn allocation_fits(
    function: &BcFunction,
    instruction: Instr,
    registers: &[u64],
    heap: &Heap,
    budget: &EvaluationBudget,
) -> bool {
    let allocation = match instruction.op {
        Opcode::Alloca | Opcode::HallocC | Opcode::HallocU => function
            .allocations
            .get(instruction.bx() as usize)
            .map(|shape| (shape.size as usize, shape.align.max(1) as usize)),
        Opcode::HallocR => Some((
            registers[instruction.b as usize] as usize,
            (registers[instruction.c as usize] as usize).max(1),
        )),
        _ => None,
    };
    allocation.is_none_or(|(size, align)| heap.allocation_fits(size, align, budget.memory_bytes))
}
