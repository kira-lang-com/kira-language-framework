use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const ORDER_MASK: u64 = 0x7 << 20;
const FAILURE_ORDER_MASK: u64 = 0x7 << 23;
const SCOPE_MASK: u64 = 0x7 << 26;
const WEAK: u64 = 1 << 29;

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    let relative = opcode - 0x200;
    let allowed = match relative {
        0 | 1 | 4 => ORDER_MASK | SCOPE_MASK,
        2 => ORDER_MASK | SCOPE_MASK | SELECTOR_MASK,
        3 => ORDER_MASK | SCOPE_MASK | FAILURE_ORDER_MASK | WEAK,
        5 | 6 => 0,
        _ => return Err(malformed(base + *cursor, "unsupported atomic opcode")),
    };
    if modifiers & !allowed != 0 {
        return Err(malformed(
            base + *cursor,
            "invalid atomic instruction modifiers",
        ));
    }
    let order = (modifiers & ORDER_MASK) >> 20;
    let failure = (modifiers & FAILURE_ORDER_MASK) >> 23;
    let scope = (modifiers & SCOPE_MASK) >> 26;
    if scope > 5 {
        return Err(malformed(base + *cursor, "invalid atomic scope"));
    }
    if order > 5 {
        return Err(malformed(base + *cursor, "invalid atomic memory order"));
    }
    match relative {
        0 => {
            if !matches!(order, 0..=2) {
                return Err(malformed(
                    base + *cursor,
                    "atomic.load order cannot release",
                ));
            }
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(1)
        }
        1 => {
            if !matches!(order, 0 | 1 | 3) {
                return Err(malformed(
                    base + *cursor,
                    "atomic.store order cannot acquire",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(0)
        }
        2 => {
            if ((modifiers & SELECTOR_MASK) >> 48) > 18 {
                return Err(malformed(
                    base + *cursor,
                    "invalid atomic.rmw operation selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(1)
        }
        3 => {
            if !matches!(failure, 0 | 1 | 2 | 5) {
                return Err(malformed(
                    base + *cursor,
                    "invalid compare-exchange failure order",
                ));
            }
            let effective_success = if order == 0 { 5 } else { order };
            let effective_failure = if failure == 0 {
                match effective_success {
                    1 | 3 => 1,
                    2 | 4 => 2,
                    5 => 5,
                    _ => unreachable!(),
                }
            } else {
                failure
            };
            let failure_allowed = match effective_success {
                1 | 3 => effective_failure == 1,
                2 | 4 => matches!(effective_failure, 1 | 2),
                5 => matches!(effective_failure, 1 | 2 | 5),
                _ => false,
            };
            if !failure_allowed {
                return Err(malformed(
                    base + *cursor,
                    "failure order is stronger than success order",
                ));
            }
            read_values(bytes, cursor, base, current_value, 3, operands)?;
            Ok(1)
        }
        4 => {
            if failure != 0 || modifiers & WEAK != 0 {
                return Err(malformed(
                    base + *cursor,
                    "fence has compare-exchange modifiers",
                ));
            }
            read_values(bytes, cursor, base, current_value, 0, operands)?;
            Ok(0)
        }
        5 => {
            read_values(bytes, cursor, base, current_value, 3, operands)?;
            Ok(1)
        }
        6 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(1)
        }
        _ => Err(malformed(base + *cursor, "unsupported atomic opcode")),
    }
}
