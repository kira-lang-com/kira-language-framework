use super::super::decoded::{DecodedImmediateBinary, DecodedInstr, ImmediateBinaryKind};
use super::super::frames::{Frame, unwind};
use super::Outcome;
use super::helpers::{join_i32, read_register, write_register};
use super::memory::execute_decoded_memory;
use klf_rt::Heap;

#[inline(always)]
pub(super) fn execute<const METERED: bool, const MEMORY: bool>(
    operation: Option<DecodedImmediateBinary>,
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
    write_register(
        &mut active.registers,
        operation.constant_register,
        join_i32(operation.low, operation.high) as u64,
    );
    if METERED {
        *pc += 1;
        return None;
    }
    let lhs = read_register(&active.registers, operation.b);
    let rhs = read_register(&active.registers, operation.c);
    let result = match operation.kind {
        ImmediateBinaryKind::Add => lhs.wrapping_add(rhs),
        ImmediateBinaryKind::Mul => lhs.wrapping_mul(rhs),
    };
    write_register(&mut active.registers, operation.a, result);
    *pc += 2;
    if MEMORY {
        macro_rules! memory {
            ($bytes:literal, $load:literal, $a:ident, $b:ident) => {
                execute_decoded_memory::<$bytes, $load>($a, $b, active, callers, heap, pc)
            };
        }
        return match operation.memory {
            Some(DecodedInstr::Load8 { a, b }) => memory!(1, true, a, b),
            Some(DecodedInstr::Load16 { a, b }) => memory!(2, true, a, b),
            Some(DecodedInstr::Load32 { a, b }) => memory!(4, true, a, b),
            Some(DecodedInstr::Load64 { a, b }) => memory!(8, true, a, b),
            Some(DecodedInstr::Store8 { a, b }) => memory!(1, false, a, b),
            Some(DecodedInstr::Store16 { a, b }) => memory!(2, false, a, b),
            Some(DecodedInstr::Store32 { a, b }) => memory!(4, false, a, b),
            Some(DecodedInstr::Store64 { a, b }) => memory!(8, false, a, b),
            _ => Some(unwind(
                active,
                callers,
                heap,
                Outcome::Trap("KLF-VM-0001".to_owned()),
            )),
        };
    }
    None
}
