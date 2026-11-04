use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_tvisa_type};
use tir_types::{AddressSpace, Type};
use tvisa_format::{ParameterItem, ResultItem, TextSignature};

use crate::calls::opcode_end;

const VFY5: Code = Code::new(Dialect::Tvisa, Area::Verifier, 5);
const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_indirect_callee(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    is_indirect: bool,
) -> Vec<Diagnostic> {
    if !is_indirect {
        return Vec::new();
    }
    let start = opcode_end(instruction);
    let Some(Token {
        kind: TokenKind::Punctuation('%'),
        ..
    }) = instruction.tokens.get(start)
    else {
        return Vec::new();
    };
    let Some(Token {
        kind: TokenKind::Atom(name),
        span,
    }) = instruction.tokens.get(start + 1)
    else {
        return Vec::new();
    };
    let Some(actual) = value_types.get(name) else {
        return Vec::new();
    };
    if *actual == Type::Code {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                VFY3,
                format!("indirect callee %{name} has type {actual}; expected code"),
            )
            .with_primary_span(*span),
        ]
    }
}

pub(super) fn infer_instruction_results(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    slot_types: &HashMap<usize, Type>,
    gpu_function: bool,
) -> Vec<(String, Type)> {
    if instruction.results.is_empty() {
        return Vec::new();
    }
    let inferred = match instruction.opcode.as_str() {
        "const.bool" | "target.has" => vec![Type::Integer(1)],
        "icmp" | "fcmp" => value_operand_types(instruction, value_types)
            .first()
            .map(|ty| lane_result_type(ty, Type::Integer(1)))
            .into_iter()
            .collect(),
        "fclass" => value_operand_types(instruction, value_types)
            .first()
            .map(|ty| lane_result_type(ty, Type::Integer(32)))
            .into_iter()
            .collect(),
        "fp.flags.read" | "setjmp" => vec![Type::Integer(32)],
        "func.ref" | "foreign.addr" | "foreign.entry" | "code.load" | "code.fromaddr" => {
            vec![Type::Code]
        }
        "global.addr" => vec![if gpu_function {
            Type::Pointer(AddressSpace::Constant)
        } else {
            generic_pointer()
        }],
        "tls.addr" | "stack.alloc.dyn" | "stack.save" | "thread.pointer" | "frame.address"
        | "return.address" | "kernel.percpu.addr" | "foreign.data" | "code.addr" | "block.addr" => {
            vec![generic_pointer()]
        }
        "slot.addr" => slot_id(instruction)
            .and_then(|id| slot_types.get(&id))
            .cloned()
            .into_iter()
            .collect(),
        "ptr.offset" => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .into_iter()
            .collect(),
        "gpu.entry.ref" => vec![Type::Integer(64)],
        "const.sampler" => vec![Type::Sampler],
        "ptr.diff" | "target.pagesize" | "vscale.fixed" | "vec.vscale" => {
            vec![Type::Integer(64)]
        }
        "landingpad" => vec![generic_pointer(), Type::Integer(32)],
        "const.null" => vec![null_pointer_type(instruction)],
        "raw.load" | "atomic.load" | "mmio.load" | "va.arg" | "const.int" | "const.float"
        | "const.zero" | "const.vec" | "const.bytes" | "const.poison" => {
            explicit_first_type(source, instruction)
                .into_iter()
                .collect()
        }
        "atomic.rmw" => value_operand_types(instruction, value_types)
            .get(1)
            .cloned()
            .into_iter()
            .collect(),
        "atomic.cmpxchg" => value_operand_types(instruction, value_types)
            .get(1)
            .cloned()
            .map(|ty| vec![ty, Type::Integer(1)])
            .unwrap_or_default(),
        "select" => value_operand_types(instruction, value_types)
            .get(1)
            .cloned()
            .into_iter()
            .collect(),
        "freeze" => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .into_iter()
            .collect(),
        "add.ovf" | "sub.ovf" | "mul.ovf" => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .map(|ty| vec![ty.clone(), lane_result_type(&ty, Type::Integer(1))])
            .unwrap_or_default(),
        "frexp" => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .map(|ty| vec![ty.clone(), lane_result_type(&ty, Type::Integer(32))])
            .unwrap_or_default(),
        "trunc" | "zext" | "sext" | "fptrunc" | "fpext" | "sitofp" | "uitofp" | "fptosi"
        | "fptoui" | "ptrtoint" | "inttoptr" | "bitcast" | "fcvt" => {
            type_after_atom(source, instruction, "to")
                .into_iter()
                .collect()
        }
        opcode if opcode.starts_with("math.") => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .into_iter()
            .collect(),
        opcode if opcode.starts_with("vec.") => {
            crate::vector_inference::infer_result_types(source, instruction, value_types)
        }
        "intrinsic" => crate::intrinsic_types::infer_result_types(
            instruction,
            &value_operand_types(instruction, value_types),
        ),
        "gpu.builtin"
        | "gpu.spec_const"
        | "gpu.ddx"
        | "gpu.ddy"
        | "gpu.fwidth"
        | "quad.broadcast"
        | "quad.swap"
        | "quad.all"
        | "quad.any"
        | "subgroup.elect"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.broadcast"
        | "subgroup.broadcast_first"
        | "subgroup.ballot"
        | "subgroup.inverse_ballot"
        | "subgroup.ballot_bit_count"
        | "subgroup.ballot_find_lsb"
        | "subgroup.ballot_find_msb"
        | "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate"
        | "subgroup.reduce"
        | "subgroup.scan_inclusive"
        | "subgroup.scan_exclusive" => crate::gpu_types::infer_result_types(
            source,
            instruction,
            &value_operand_types(instruction, value_types),
        ),
        "gpu.call.vft" => {
            crate::gpu_vft_types::infer_result_types(&value_operand_types(instruction, value_types))
        }
        opcode if is_gpu_instruction(opcode) => crate::gpu_types::infer_result_types(
            source,
            instruction,
            &value_operand_types(instruction, value_types),
        ),
        opcode if is_value_preserving(opcode) => value_operand_types(instruction, value_types)
            .first()
            .cloned()
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    instruction.results.iter().cloned().zip(inferred).collect()
}

fn is_gpu_instruction(opcode: &str) -> bool {
    [
        "gpu.",
        "quad.",
        "subgroup.",
        "tex.",
        "object.",
        "mesh.",
        "rq.",
        "rt.",
        "coop.",
        "tensor.",
        "tile.",
        "frag.",
        "node.",
    ]
    .iter()
    .any(|prefix| opcode.starts_with(prefix))
}

pub(super) fn signature_parameter_types(
    signature: &TextSignature,
    parameter_count: usize,
) -> Vec<Type> {
    signature
        .parameters
        .iter()
        .take(parameter_count)
        .flat_map(parameter_types)
        .collect()
}

fn parameter_types(parameter: &ParameterItem) -> Vec<Type> {
    match parameter {
        ParameterItem::Leaf(ty) => vec![ty.clone()],
        ParameterItem::Group(group) => group
            .members
            .iter()
            .map(|member| member.ty.clone())
            .collect(),
        ParameterItem::ByRef { .. }
        | ParameterItem::Sret { .. }
        | ParameterItem::Context
        | ParameterItem::CAggregate(_) => vec![generic_pointer()],
    }
}

pub(super) fn signature_result_types(signature: &TextSignature) -> Vec<Type> {
    let mut results = signature
        .results
        .iter()
        .flat_map(result_types)
        .collect::<Vec<_>>();
    if signature.fails {
        results.push(generic_pointer());
    }
    results
}

fn result_types(result: &ResultItem) -> Vec<Type> {
    match result {
        ResultItem::Leaf(ty) => vec![ty.clone()],
        ResultItem::Group(group) => group
            .members
            .iter()
            .map(|member| member.ty.clone())
            .collect(),
    }
}

fn type_after_atom(source: &str, instruction: &TextInstruction, marker: &str) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = tokens
        .iter()
        .position(|token| atom(token) == Some(marker))?
        + 1;
    let first = tokens.get(start)?;
    let mut depth = if is_punctuation(first, '[') { 1i32 } else { 0 };
    let mut end = None;
    for token in tokens.iter().skip(start + 1) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '(' => depth += 1,
                '>' | ')' => depth -= 1,
                '[' if depth == 0 => {
                    end = Some(token.span.start);
                    break;
                }
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
    }
    let end = end.or_else(|| tokens.last().map(|token| token.span.end))?;
    parse_tvisa_type(source.get(first.span.start..end)?).ok()
}

pub(super) fn verify_runtime_call(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Option<(Vec<Diagnostic>, Vec<(String, Type)>)> {
    let (expected_arguments, expected_results) =
        crate::runtime::runtime_call_signature(instruction)?;
    let start = opcode_end(instruction);
    let mut diagnostics = Vec::new();
    let Some(open) = (start..instruction.tokens.len()).find(|index| {
        instruction
            .tokens
            .get(*index)
            .is_some_and(|token| is_punctuation(token, '('))
    }) else {
        diagnostics.push(
            Diagnostic::error(VFY5, "call.runtime is missing its argument list")
                .with_primary_span(instruction.span),
        );
        let typed_results = instruction
            .results
            .iter()
            .cloned()
            .zip(expected_results)
            .collect();
        return Some((diagnostics, typed_results));
    };
    let Some(close) = matching_paren(&instruction.tokens, open) else {
        diagnostics.push(
            Diagnostic::error(VFY5, "call.runtime has an unterminated argument list")
                .with_primary_span(instruction.span),
        );
        let typed_results = instruction
            .results
            .iter()
            .cloned()
            .zip(expected_results)
            .collect();
        return Some((diagnostics, typed_results));
    };
    let (arguments, valid_arguments) = value_operands(&instruction.tokens, open, close);
    if !valid_arguments {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                "runtime arguments must be SSA values separated by commas",
            )
            .with_primary_span(instruction.span),
        );
    }
    if arguments.len() != expected_arguments.len() {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                format!(
                    "call.runtime has {} arguments; runtime entry requires {} leaves",
                    arguments.len(),
                    expected_arguments.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
    }
    for (argument, expected_type) in arguments.iter().zip(&expected_arguments) {
        if let Some(actual_type) = value_types.get(&argument.0)
            && actual_type != expected_type
        {
            diagnostics.push(
                Diagnostic::error(
                    VFY5,
                    format!(
                        "call.runtime argument %{} has type {actual_type}, expected {expected_type}",
                        argument.0
                    ),
                )
                .with_primary_span(argument.1),
            );
        }
    }
    if instruction.results.len() != expected_results.len() {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                format!(
                    "call.runtime defines {} results; runtime entry yields {} leaves",
                    instruction.results.len(),
                    expected_results.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
    }
    let typed_results = instruction
        .results
        .iter()
        .cloned()
        .zip(expected_results)
        .collect();
    Some((diagnostics, typed_results))
}

fn is_value_preserving(opcode: &str) -> bool {
    [
        "add",
        "sub",
        "mul",
        "mulh",
        "udiv",
        "sdiv",
        "urem",
        "srem",
        "fdiv",
        "frem",
        "fadd",
        "fsub",
        "fmul",
        "fmuladd",
        "min",
        "max",
        "and",
        "or",
        "xor",
        "shl",
        "lshr",
        "ashr",
        "rotl",
        "rotr",
        "fshl",
        "fshr",
        "smin",
        "smax",
        "umin",
        "umax",
        "clmul",
        "clmulh",
        "ctlz",
        "cttz",
        "ctpop",
        "bswap",
        "bitreverse",
        "fmin",
        "fmax",
        "fminnum",
        "fmaxnum",
        "fcopysign",
        "fma",
        "ffloor",
        "fceil",
        "ftrunc",
        "fround",
        "froundeven",
        "ldexp",
        "abs",
        "neg",
        "not",
    ]
    .iter()
    .any(|prefix| opcode == *prefix || opcode.starts_with(&format!("{prefix}.")))
}

fn value_operand_types(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Vec<Type> {
    let start = opcode_end(instruction);
    instruction.tokens[start..]
        .windows(2)
        .filter_map(|window| {
            if is_punctuation(&window[0], '%') {
                atom(&window[1])
                    .and_then(|name| value_types.get(name))
                    .cloned()
            } else {
                None
            }
        })
        .collect()
}

fn explicit_first_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let start = opcode_end(instruction);
    let first = instruction.tokens.get(start)?;
    let mut depth = 0i32;
    let mut end = None;
    for token in instruction.tokens.iter().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => {
                    end = Some(token.span.start);
                    break;
                }
                _ => {}
            }
        }
    }
    let end = end.or_else(|| instruction.tokens.last().map(|token| token.span.end))?;
    parse_tvisa_type(source.get(first.span.start..end)?).ok()
}

fn value_operands(
    tokens: &[Token],
    open: usize,
    close: usize,
) -> (Vec<(String, tir_diag::Span)>, bool) {
    let mut result = Vec::new();
    let mut start = open + 1;
    let mut valid = true;
    for index in open + 1..close {
        if is_punctuation(&tokens[index], ',') {
            valid &= push_value(&mut result, &tokens[start..index]);
            start = index + 1;
        }
    }
    if start < close {
        valid &= push_value(&mut result, &tokens[start..close]);
    } else if start != open + 1 {
        valid = false;
    }
    (result, valid)
}

fn push_value(values: &mut Vec<(String, tir_diag::Span)>, tokens: &[Token]) -> bool {
    if let Some(value) = parse_value(tokens) {
        values.push(value);
        true
    } else {
        false
    }
}

fn parse_value(tokens: &[Token]) -> Option<(String, tir_diag::Span)> {
    if tokens.len() != 2 || !is_punctuation(&tokens[0], '%') {
        return None;
    }
    let name = atom(&tokens[1])?.to_owned();
    Some((
        name,
        tir_diag::Span::new(tokens[0].span.start, tokens[1].span.end),
    ))
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

pub(super) fn generic_pointer() -> Type {
    Type::Pointer(AddressSpace::Generic)
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn slot_id(instruction: &TextInstruction) -> Option<usize> {
    let token = instruction.tokens.get(opcode_end(instruction))?;
    let value = atom(token)?;
    value.parse().ok()
}

fn null_pointer_type(instruction: &TextInstruction) -> Type {
    use AddressSpace::{Constant, Device, Generic, HitAttribute, NodeRecord, ObjectPayload};
    use AddressSpace::{RayPayload, Thread, Threadgroup, Tile};

    match instruction
        .tokens
        .get(opcode_end(instruction))
        .and_then(atom)
    {
        Some("device") => Type::Pointer(Device),
        Some("constant") => Type::Pointer(Constant),
        Some("threadgroup") => Type::Pointer(Threadgroup),
        Some("thread") => Type::Pointer(Thread),
        Some("object_payload") => Type::Pointer(ObjectPayload),
        Some("ray_payload") => Type::Pointer(RayPayload),
        Some("hit_attribute") => Type::Pointer(HitAttribute),
        Some("tile") => Type::Pointer(Tile),
        Some("node_record") => Type::Pointer(NodeRecord),
        Some("generic") | None => Type::Pointer(Generic),
        _ => Type::Pointer(Generic),
    }
}

fn lane_result_type(input: &Type, element: Type) -> Type {
    match input {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(element),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(element)),
        _ => element,
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
