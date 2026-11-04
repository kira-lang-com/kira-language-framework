use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::Type;

use super::{ballot_type, error, is_float_value, is_scalar_value};

const OPERATORS: [&str; 15] = [
    "add.s", "add.u", "add.f", "mul.s", "mul.u", "mul.f", "min.s", "min.u", "min.f", "max.s",
    "max.u", "max.f", "and", "or", "xor",
];

pub(in crate::gpu_profile) fn explicit_cluster_sizes(instruction: &TextInstruction) -> Vec<u64> {
    words_after_opcode(instruction)
        .windows(2)
        .filter(|words| words[0] == "cluster")
        .filter_map(|words| words[1].parse::<u64>().ok())
        .collect()
}

pub(super) fn verify(
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<tir_diag::Diagnostic> {
    let mut diagnostics = Vec::new();
    match instruction.opcode.as_str() {
        "gpu.ddx" | "gpu.ddy" | "gpu.fwidth" => {
            if operands.first().is_some_and(|ty| !is_float_value(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} requires a floating-point value", instruction.opcode),
                );
            }
        }
        "quad.broadcast" | "subgroup.broadcast" => {
            check_lane(instruction, operands, &mut diagnostics, "broadcast lane")
        }
        "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate" => {
            check_lane(instruction, operands, &mut diagnostics, "lane or offset")
        }
        "quad.all" | "quad.any" | "subgroup.all" | "subgroup.any" | "subgroup.ballot" => {
            if operands.first().is_some_and(|ty| ty != &Type::Integer(1)) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} requires an i1 value", instruction.opcode),
                );
            }
        }
        "subgroup.all_equal" => {
            if operands.first().is_some_and(|ty| !is_scalar_value(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "subgroup.all_equal requires a scalar value",
                );
            }
        }
        "subgroup.inverse_ballot"
        | "subgroup.ballot_bit_count"
        | "subgroup.ballot_find_lsb"
        | "subgroup.ballot_find_msb" => {
            if operands.first().is_some_and(|ty| ty != &ballot_type()) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} requires vec<4 x i32>", instruction.opcode),
                );
            }
        }
        "subgroup.reduce" | "subgroup.scan_inclusive" | "subgroup.scan_exclusive" => {
            verify_reduction(instruction, operands, &mut diagnostics);
        }
        _ => {}
    }
    diagnostics
}

fn check_lane(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
    name: &str,
) {
    if operands.get(1).is_some_and(|ty| ty != &Type::Integer(32)) {
        error(
            diagnostics,
            instruction,
            format!("{} {name} must have type i32", instruction.opcode),
        );
    }
}

fn verify_reduction(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    let words = words_after_opcode(instruction);
    let mut selected = Vec::new();
    let mut cluster = None;
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        if OPERATORS.contains(&word) {
            selected.push(word);
        } else if word == "cluster" {
            if cluster.is_some() {
                error(
                    diagnostics,
                    instruction,
                    "subgroup reduction may specify cluster size only once",
                );
            }
            let size = words
                .get(index + 1)
                .and_then(|word| word.parse::<u64>().ok());
            if let Some(size) = size {
                if size == 0 || size > 128 || !size.is_power_of_two() {
                    error(
                        diagnostics,
                        instruction,
                        "subgroup cluster size must be a power of two from 1 through 128",
                    );
                }
                cluster = Some(size);
                index += 1;
            } else {
                error(
                    diagnostics,
                    instruction,
                    "subgroup cluster requires a power-of-two size",
                );
            }
        } else {
            error(
                diagnostics,
                instruction,
                format!("unsupported subgroup reduction option `{word}`"),
            );
        }
        index += 1;
    }
    if selected.len() != 1 {
        error(
            diagnostics,
            instruction,
            "subgroup reduction must select exactly one operator",
        );
        return;
    }
    let compatible = match selected[0] {
        "add.f" | "mul.f" | "min.f" | "max.f" => operands
            .first()
            .is_some_and(|ty| matches!(ty, Type::Float(_))),
        _ => operands
            .first()
            .is_some_and(|ty| matches!(ty, Type::Integer(_))),
    };
    if !compatible {
        error(
            diagnostics,
            instruction,
            "subgroup reduction operator is incompatible with its scalar operand",
        );
    }
}

fn words_after_opcode(instruction: &TextInstruction) -> Vec<String> {
    let mut words = Vec::new();
    let mut index = crate::ssa::text_instruction_operands_start(&instruction.tokens);
    while index < instruction.tokens.len() {
        if is_punctuation(&instruction.tokens[index], '%') {
            index += 2;
            continue;
        }
        if let Some(value) = instruction.tokens.get(index).and_then(atom) {
            if instruction
                .tokens
                .get(index + 1)
                .is_some_and(|token| is_punctuation(token, '.'))
                && let Some(suffix) = instruction.tokens.get(index + 2).and_then(atom)
            {
                words.push(format!("{value}.{suffix}"));
                index += 3;
                continue;
            }
            words.push(value.to_owned());
        }
        index += 1;
    }
    words
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
