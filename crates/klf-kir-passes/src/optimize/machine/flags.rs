//! Machine overflow flags (section 14.1, tier 2). A whole-word sum checked
//! by the sign of `(a ^ r) & (b ^ r)`, or a difference by that of
//! `(a ^ b) & (a ^ r)`, becomes `add.ovf` or `sub.ovf`, whose flag the
//! backend reads from the machine instead of computing it.

use super::super::Context;
use super::super::util::{constants, fresh, rename_uses, use_counts};
use klf_core::Name;
use klf_kir::{BinaryOperator, Function, Instruction, IntegerPredicate, Operation, Overflow, Type};
use rustc_hash::FxHashMap as HashMap;

/// Rewrites every overflow test of the function the machine can answer.
pub(super) fn machine_flags(function: &mut Function, context: &Context<'_>) -> bool {
    let tests = overflow_tests(function, context);
    if tests.is_empty() {
        return false;
    }
    let mut flags: HashMap<Name, Name> = HashMap::default();
    for (_, result, _) in &tests {
        flags
            .entry(result.clone())
            .or_insert_with(|| fresh("overflowed"));
    }
    let counts = use_counts(function);
    let mut branched: HashMap<Name, usize> = HashMap::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let Operation::ConditionalBranch(branch) = &instruction.operation {
            *branched.entry(branch.condition.clone()).or_default() += 1;
        }
    }
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut swapped: HashMap<Name, Name> = HashMap::default();
    let mut negated: HashMap<Name, Name> = HashMap::default();
    for (compared, result, exact) in tests {
        let flag = flags[&result].clone();
        if !exact {
            renames.insert(compared, flag);
        } else if counts.get(&compared) == branched.get(&compared) {
            swapped.insert(compared, flag);
        } else {
            negated.insert(compared, flag);
        }
    }
    for block in &mut function.blocks {
        let mut instructions = Vec::with_capacity(block.instructions.len());
        for mut instruction in std::mem::take(&mut block.instructions) {
            let flag = match instruction.results.as_slice() {
                [result] => flags.get(result).cloned(),
                _ => None,
            };
            if let (
                Some(flag),
                Operation::Binary {
                    operator, lhs, rhs, ..
                },
            ) = (flag, &instruction.operation)
            {
                instruction.operation = Operation::BinaryOverflow {
                    operator: *operator,
                    signed: true,
                    lhs: lhs.clone(),
                    rhs: rhs.clone(),
                };
                instruction.results.push(flag);
            }
            if let Some(flag) = instruction
                .results
                .first()
                .and_then(|name| negated.get(name))
            {
                let no = fresh("false");
                instructions.push(Instruction {
                    results: vec![no.clone()],
                    operation: Operation::ConstBool(false),
                    debug_location: None,
                    span: instruction.span,
                });
                instruction.operation = Operation::IntegerCompare {
                    predicate: IntegerPredicate::Eq,
                    lhs: flag.clone(),
                    rhs: no,
                };
            }
            if let Operation::ConditionalBranch(branch) = &mut instruction.operation
                && let Some(flag) = swapped.get(&branch.condition)
            {
                branch.condition = flag.clone();
                std::mem::swap(&mut branch.then_target, &mut branch.else_target);
            }
            instructions.push(instruction);
        }
        block.instructions = instructions;
    }
    rename_uses(function, &renames);
    true
}

/// The overflow tests of the function: the compare, the checked result,
/// and whether the compare answers that the result is exact.
fn overflow_tests(function: &Function, context: &Context<'_>) -> Vec<(Name, Name, bool)> {
    let known = constants(function);
    let types = klf_kir::infer_value_types_in(function, context.index);
    let mut definitions: HashMap<Name, Operation> = HashMap::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let [result] = instruction.results.as_slice() {
            definitions.insert(result.clone(), instruction.operation.clone());
        }
    }
    let mut tests: Vec<(Name, Name, bool)> = Vec::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        let (
            [compared],
            Operation::IntegerCompare {
                predicate,
                lhs,
                rhs,
            },
        ) = (instruction.results.as_slice(), &instruction.operation)
        else {
            continue;
        };
        let exact = match predicate {
            IntegerPredicate::SignedGe => true,
            IntegerPredicate::SignedLt => false,
            _ => continue,
        };
        if known.get(rhs).is_none_or(|&(_, value)| value != 0) {
            continue;
        }
        if let Some(result) = checked(lhs, &definitions)
            && types.get(&result).is_some_and(is_word)
        {
            tests.push((compared.clone(), result, exact));
        }
    }
    tests
}

/// The whole-word sum or difference `value`'s sign tests for overflow.
fn checked(value: &Name, definitions: &HashMap<Name, Operation>) -> Option<Name> {
    let wrapped = |name: &Name, wanted: BinaryOperator| match definitions.get(name)? {
        Operation::Binary {
            operator,
            overflow: Overflow::Wrap,
            lhs,
            rhs,
            ..
        } if *operator == wanted => Some((lhs, rhs)),
        _ => None,
    };
    let (first, second) = wrapped(value, BinaryOperator::And)?;
    let first = wrapped(first, BinaryOperator::Xor)?;
    let second = wrapped(second, BinaryOperator::Xor)?;
    let pair =
        |(x, y): (&Name, &Name), (p, q): (&Name, &Name)| (x == p && y == q) || (x == q && y == p);
    for (one, other) in [(first, second), (second, first)] {
        // `(a ^ r) & (b ^ r)` with `r = a + b`.
        for result in [one.0, one.1] {
            let operand = if one.0 == result { one.1 } else { one.0 };
            if let Some(sum) = wrapped(result, BinaryOperator::Add)
                && pair(sum, (operand, other_operand(other, result)?))
            {
                return Some(result.clone());
            }
        }
        // `(a ^ b) & (a ^ r)` with `r = a - b`.
        for result in [other.0, other.1] {
            let start = if other.0 == result { other.1 } else { other.0 };
            if let Some((lhs, rhs)) = wrapped(result, BinaryOperator::Sub)
                && lhs == start
                && pair(one, (lhs, rhs))
            {
                return Some(result.clone());
            }
        }
    }
    None
}

/// The operand of `xor` beside `result`, when `result` is one of its two.
fn other_operand<'a>(xor: (&'a Name, &'a Name), result: &Name) -> Option<&'a Name> {
    if xor.0 == result {
        Some(xor.1)
    } else if xor.1 == result {
        Some(xor.0)
    } else {
        None
    }
}

/// Whether the flag of `ty` is one a backend reads from the machine.
fn is_word(ty: &Type) -> bool {
    matches!(ty, Type::Integer(32 | 64))
}
