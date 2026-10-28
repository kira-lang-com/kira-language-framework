//! Constant folding and copy propagation (section 14.1, O1). A fold never
//! removes a trap: an operation whose constant operands would trap stays.

use super::Context;
use super::util::{constants, is_scalar, rename_uses};
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, Function, IntegerPredicate, Operation, Overflow, Type};
use rustc_hash::FxHashMap as HashMap;

/// `value` read as a signed integer of `width` bits.
fn signed(value: i128, width: u16) -> i128 {
    if width >= 128 {
        return value;
    }
    let shift = 128 - u32::from(width);
    (value << shift) >> shift
}

/// `value` read as an unsigned integer of `width` bits.
fn unsigned(value: i128, width: u16) -> i128 {
    if width >= 128 {
        return value;
    }
    value & ((1i128 << width) - 1)
}

/// The form a constant of `width` bits is written in: zero-extended below 64
/// bits, as backends lower `i1` to a byte.
fn canonical(value: i128, width: u16) -> i128 {
    if width < 64 {
        unsigned(value, width)
    } else {
        signed(value, width)
    }
}

fn fits(value: i128, width: u16, is_signed: bool) -> bool {
    if is_signed {
        signed(value, width) == value
    } else {
        unsigned(value, width) == value
    }
}

fn binary(
    operator: BinaryOperator,
    overflow: Overflow,
    is_signed: bool,
    width: u16,
    lhs: i128,
    rhs: i128,
) -> Option<i128> {
    use BinaryOperator as B;
    if width > 64 || overflow == Overflow::Saturate {
        return None;
    }
    let read = |value| {
        if is_signed {
            signed(value, width)
        } else {
            unsigned(value, width)
        }
    };
    let (left, right) = (read(lhs), read(rhs));
    let exact = match operator {
        B::Add => left + right,
        B::Sub => left - right,
        B::Mul => left.checked_mul(right)?,
        B::SignedDiv | B::UnsignedDiv | B::SignedRem | B::UnsignedRem => {
            let (left, right) = if matches!(operator, B::SignedDiv | B::SignedRem) {
                (signed(lhs, width), signed(rhs, width))
            } else {
                (unsigned(lhs, width), unsigned(rhs, width))
            };
            if right == 0 {
                return None;
            }
            let result = if matches!(operator, B::SignedDiv | B::UnsignedDiv) {
                left / right
            } else {
                left % right
            };
            if matches!(operator, B::SignedDiv) && !fits(result, width, true) {
                return None;
            }
            return Some(canonical(result, width));
        }
        B::And => lhs & rhs,
        B::Or => lhs | rhs,
        B::Xor => lhs ^ rhs,
        B::ShiftLeft | B::LogicalShiftRight | B::ArithmeticShiftRight => {
            let amount = unsigned(rhs, width);
            if amount >= i128::from(width) {
                return None;
            }
            let amount = u32::try_from(amount).ok()?;
            let shifted = match operator {
                B::ShiftLeft => unsigned(lhs, width) << amount,
                B::LogicalShiftRight => unsigned(lhs, width) >> amount,
                _ => signed(lhs, width) >> amount,
            };
            return Some(canonical(shifted, width));
        }
        B::RotateLeft | B::RotateRight => return None,
    };
    if overflow == Overflow::Trap
        && matches!(operator, B::Add | B::Sub | B::Mul)
        && !fits(exact, width, is_signed)
    {
        return None;
    }
    Some(canonical(exact, width))
}

fn compare(predicate: IntegerPredicate, width: u16, lhs: i128, rhs: i128) -> bool {
    use IntegerPredicate as P;
    let (sl, sr) = (signed(lhs, width), signed(rhs, width));
    let (ul, ur) = (unsigned(lhs, width), unsigned(rhs, width));
    match predicate {
        P::Eq => ul == ur,
        P::Ne => ul != ur,
        P::SignedLt => sl < sr,
        P::SignedLe => sl <= sr,
        P::SignedGt => sl > sr,
        P::SignedGe => sl >= sr,
        P::UnsignedLt => ul < ur,
        P::UnsignedLe => ul <= ur,
        P::UnsignedGt => ul > ur,
        P::UnsignedGe => ul >= ur,
    }
}

/// The operand an operation with one constant operand answers unchanged:
/// `x + 0`, `x * 1`, `x | 0` and the like.
fn identity(operator: BinaryOperator, width: u16, constant: i128, on_right: bool) -> bool {
    use BinaryOperator as B;
    let all_ones = signed(-1, width);
    match operator {
        B::Add | B::Or | B::Xor => constant == 0,
        B::Sub | B::ShiftLeft | B::LogicalShiftRight | B::ArithmeticShiftRight => {
            on_right && constant == 0
        }
        B::Mul => constant == 1,
        B::SignedDiv | B::UnsignedDiv => on_right && constant == 1,
        B::And => signed(constant, width) == all_ones,
        _ => false,
    }
}

enum Same {
    Zero(u16),
    Operand,
}

/// What `x op x` is, when it is known whatever `x` holds.
fn same_operands(operator: BinaryOperator, ty: Option<&Type>) -> Option<Same> {
    use BinaryOperator as B;
    match (operator, ty?) {
        (B::Xor | B::Sub, Type::Integer(width)) => Some(Same::Zero(*width)),
        (B::And | B::Or, Type::Integer(_)) => Some(Same::Operand),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn fold(function: &mut Function, context: &Context<'_>) -> bool {
    let mut known = constants(function);
    let types = context.types(function);
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut changed = false;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let result = instruction.results.first().cloned();
            let constant = |name: &Name| known.get(name).copied();
            let folded: Option<Operation> = match &instruction.operation {
                Operation::Binary {
                    operator,
                    overflow,
                    signed: is_signed,
                    lhs,
                    rhs,
                } => match (constant(lhs), constant(rhs)) {
                    (Some((width, left)), Some((_, right))) => {
                        binary(*operator, *overflow, *is_signed, width, left, right).map(|value| {
                            Operation::ConstInt {
                                ty: Type::Integer(width),
                                value,
                            }
                        })
                    }
                    (Some((width, left)), None) if identity(*operator, width, left, false) => {
                        renames.insert(result.clone().expect("a result"), rhs.clone());
                        Some(Operation::Nop)
                    }
                    (None, Some((width, right))) if identity(*operator, width, right, true) => {
                        renames.insert(result.clone().expect("a result"), lhs.clone());
                        Some(Operation::Nop)
                    }
                    (Some((width, 0)), _) | (_, Some((width, 0)))
                        if matches!(operator, BinaryOperator::And | BinaryOperator::Mul) =>
                    {
                        Some(Operation::ConstInt {
                            ty: Type::Integer(width),
                            value: 0,
                        })
                    }
                    _ if lhs == rhs => {
                        same_operands(*operator, types.get(lhs)).map(|same| match same {
                            Same::Zero(width) => Operation::ConstInt {
                                ty: Type::Integer(width),
                                value: 0,
                            },
                            Same::Operand => {
                                renames.insert(result.clone().expect("a result"), lhs.clone());
                                Operation::Nop
                            }
                        })
                    }
                    _ => None,
                },
                Operation::IntegerCompare {
                    predicate,
                    lhs,
                    rhs,
                } => match (constant(lhs), constant(rhs)) {
                    (Some((width, left)), Some((_, right))) => Some(Operation::ConstInt {
                        ty: Type::Integer(1),
                        value: i128::from(compare(*predicate, width, left, right)),
                    }),
                    _ if lhs == rhs => Some(Operation::ConstInt {
                        ty: Type::Integer(1),
                        value: i128::from(matches!(
                            predicate,
                            IntegerPredicate::Eq
                                | IntegerPredicate::SignedLe
                                | IntegerPredicate::SignedGe
                                | IntegerPredicate::UnsignedLe
                                | IntegerPredicate::UnsignedGe
                        )),
                    }),
                    _ => None,
                },
                Operation::Select {
                    condition,
                    then_value,
                    else_value,
                } => {
                    let chosen = match constant(condition) {
                        Some((_, value)) if value & 1 == 1 => Some(then_value),
                        Some(_) => Some(else_value),
                        None if then_value == else_value => Some(then_value),
                        None => None,
                    };
                    chosen.map(|value| {
                        renames.insert(result.clone().expect("a result"), value.clone());
                        Operation::Nop
                    })
                }
                Operation::Convert { kind, value, ty } => match (constant(value), ty) {
                    (Some((from, input)), Type::Integer(to)) => {
                        let output = match kind {
                            ConvertKind::Truncate => Some(canonical(input, *to)),
                            ConvertKind::ZeroExtend => Some(canonical(unsigned(input, from), *to)),
                            ConvertKind::SignExtend => Some(canonical(signed(input, from), *to)),
                            _ => None,
                        };
                        output.map(|value| Operation::ConstInt {
                            ty: Type::Integer(*to),
                            value,
                        })
                    }
                    _ => None,
                },
                Operation::Move(value) | Operation::Copy(value)
                    if types.get(value).is_some_and(is_scalar) =>
                {
                    renames.insert(result.clone().expect("a result"), value.clone());
                    Some(Operation::Nop)
                }
                Operation::ConditionalBranch(branch) => {
                    constant(&branch.condition).map(|(_, value)| {
                        Operation::Branch(if value & 1 == 1 {
                            branch.then_target.clone()
                        } else {
                            branch.else_target.clone()
                        })
                    })
                }
                Operation::SwitchInt(switch) => constant(&switch.value).map(|(width, value)| {
                    let target = switch
                        .cases
                        .iter()
                        .find(|(case, _)| signed(*case, width) == signed(value, width))
                        .map_or(&switch.default, |(_, target)| target);
                    Operation::Branch(target.clone())
                }),
                _ => None,
            };
            if let Some(operation) = folded {
                if matches!(operation, Operation::Nop) {
                    instruction.results.clear();
                }
                if let (Some(result), Some(value)) = (&result, super::util::const_int(&operation)) {
                    known.insert(result.clone(), value);
                }
                instruction.operation = operation;
                changed = true;
            }
        }
    }
    rename_uses(function, &renames);
    changed
}
