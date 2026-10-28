use crate::program::AllocShape;

use super::{DecodedAllocaPair, DecodedInstr, join_u32};

pub(super) fn decode_alloca_pairs(
    code: &mut [DecodedInstr],
    allocations: &[AllocShape],
) -> Vec<DecodedAllocaPair> {
    let mut pairs = Vec::new();
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::Alloca {
            a: first_a,
            index_low: first_low,
            index_high: first_high,
        } = code[pc]
        else {
            pc += 1;
            continue;
        };
        let DecodedInstr::Alloca {
            a: second_a,
            index_low: second_low,
            index_high: second_high,
        } = code[pc + 1]
        else {
            pc += 1;
            continue;
        };
        let Some(first_shape) = allocations.get(join_u32(first_low, first_high) as usize) else {
            pc += 1;
            continue;
        };
        let Some(second_shape) = allocations.get(join_u32(second_low, second_high) as usize) else {
            pc += 1;
            continue;
        };
        let Ok(index) = u32::try_from(pairs.len()) else {
            break;
        };
        pairs.push(DecodedAllocaPair {
            first_a,
            first_size: first_shape.size,
            first_align: first_shape.align,
            second_a,
            second_size: second_shape.size,
            second_align: second_shape.align,
        });
        code[pc] = DecodedInstr::AllocaPair(index);
        pc += 2;
    }
    pairs
}
