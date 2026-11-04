use crate::binary::FormatError;

use super::helpers::{malformed, read_values};
use super::{BinaryOperand, read_index};

const SELECTOR_MASK: u64 = 0xf << 48;
const FINE: u64 = 1 << 36;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    modifiers: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    let allowed = match opcode {
        0x420 | 0x421 => FINE | UNCHECKED_UNIFORMITY,
        0x424 => SELECTOR_MASK | UNCHECKED_UNIFORMITY,
        0x423 | 0x425 | 0x426 => UNCHECKED_UNIFORMITY,
        0x422 => UNCHECKED_UNIFORMITY,
        0x440..=0x452 => 0,
        _ => return Ok(None),
    };
    if modifiers & !allowed != 0 {
        return Err(malformed(
            base + start,
            "GPU collective instruction has unsupported modifiers",
        ));
    }
    let (value_count, result_count) = match opcode {
        0x420..=0x422 => (1, 1),
        0x423 => (2, 1),
        0x424 => {
            if (modifiers >> 48) & 0xf > 2 {
                return Err(malformed(base + start, "invalid quad swap direction"));
            }
            (1, 1)
        }
        0x425 | 0x426 => (1, 1),
        0x440 => (0, 1),
        0x441..=0x443 | 0x446..=0x44a => (1, 1),
        0x444 => (2, 1),
        0x445 => (1, 1),
        0x44b..=0x44f => (2, 1),
        0x450..=0x452 => (1, 1),
        _ => return Ok(None),
    };

    if value_count != 0 {
        read_values(bytes, cursor, base, current_value, value_count, operands)?;
    }
    if matches!(opcode, 0x450..=0x452) {
        let operator = read_index(bytes, cursor, base, "subgroup reduction operator")?;
        if operator > 14 {
            return Err(malformed(
                base + start,
                "invalid subgroup reduction operator",
            ));
        }
        operands.push(BinaryOperand::Index(operator));
        let cluster = read_index(bytes, cursor, base, "subgroup cluster size")?;
        if cluster != 0 && (cluster > 128 || !cluster.is_power_of_two()) {
            return Err(malformed(
                base + start,
                "subgroup cluster size must be a power of two no greater than 128",
            ));
        }
        operands.push(BinaryOperand::Index(cluster));
    }
    Ok(Some((result_count, false)))
}
