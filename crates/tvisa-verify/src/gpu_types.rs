use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::parse_tvisa_type;
use tir_format::text::{Token, TokenKind};
use tir_types::{FloatType, Type};

use crate::calls::opcode_end;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match instruction.opcode.as_str() {
        "gpu.builtin" => verify_builtin(source, instruction, &mut diagnostics),
        "gpu.spec_const" => verify_specialization_constant(source, instruction, &mut diagnostics),
        "gpu.ddx" | "gpu.ddy" | "gpu.fwidth" => {
            if operands.first().is_some_and(|ty| !is_float_value(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} requires a floating-point value", instruction.opcode),
                );
            }
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
        "quad.broadcast"
        | "subgroup.broadcast"
        | "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate" => {
            if operands.get(1).is_some_and(|ty| ty != &Type::Integer(32)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "quad and subgroup lane operands must have type i32",
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
        "subgroup.reduce" | "subgroup.scan_inclusive" | "subgroup.scan_exclusive" => {
            verify_subgroup_operation(instruction, operands, &mut diagnostics);
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
        _ => {}
    }
    diagnostics.extend(crate::gpu_texture_types::verify(instruction, operands));
    diagnostics.extend(crate::gpu_vft_types::verify(instruction, operands));
    diagnostics.extend(crate::gpu_ray_types::verify(instruction, operands));
    diagnostics.extend(crate::gpu_advanced_types::verify(
        source,
        instruction,
        operands,
    ));
    diagnostics
}

fn verify_subgroup_operation(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let operation = subgroup_operation(instruction);
    let Some(operation) = operation else {
        error(
            diagnostics,
            instruction,
            "subgroup reduction or scan requires one supported operator",
        );
        return;
    };
    let Some(value_type) = operands.first() else {
        return;
    };
    let element = match value_type {
        Type::FixedVector { element, .. } => element.as_ref(),
        scalar => scalar,
    };
    let integer = matches!(element, Type::Integer(_));
    let float = matches!(element, Type::Float(_));
    let operator_accepts_type = match operation {
        0 | 1 | 3 | 4 | 6 | 7 | 9 | 10 | 12..=14 => integer,
        2 | 5 | 8 | 11 => float,
        _ => false,
    };
    if !operator_accepts_type {
        error(
            diagnostics,
            instruction,
            "subgroup operator is not supported by the operand type",
        );
    }
    if let Some(cluster) = subgroup_cluster(instruction)
        && (cluster == 0 || cluster > 128 || !cluster.is_power_of_two())
    {
        error(
            diagnostics,
            instruction,
            "subgroup cluster size must be a power of two no greater than 128",
        );
    }
    let cluster_count = instruction
        .tokens
        .iter()
        .filter(|token| matches!(&token.kind, TokenKind::Atom(name) if name == "cluster"))
        .count();
    if cluster_count > 1 || (cluster_count == 1 && subgroup_cluster(instruction).is_none()) {
        error(
            diagnostics,
            instruction,
            "subgroup cluster must have one integer size",
        );
    }
}

fn subgroup_operation(instruction: &TextInstruction) -> Option<u8> {
    let operations = [
        "add.s", "add.u", "add.f", "mul.s", "mul.u", "mul.f", "min.s", "min.u", "min.f", "max.s",
        "max.u", "max.f", "and", "or", "xor",
    ];
    let mut found = None;
    for (index, token) in instruction.tokens.iter().enumerate() {
        let name = match &token.kind {
            TokenKind::Atom(name) => name.clone(),
            _ => continue,
        };
        if let Some(operation) = operations
            .iter()
            .position(|operation| *operation == name.as_str())
        {
            if found.replace(operation as u8).is_some() {
                return None;
            }
        }
        if index + 2 < instruction.tokens.len()
            && matches!(&token.kind, TokenKind::Atom(_))
            && matches!(
                instruction.tokens[index + 1].kind,
                TokenKind::Punctuation('.')
            )
            && matches!(&instruction.tokens[index + 2].kind, TokenKind::Atom(_))
        {
            let TokenKind::Atom(base) = &token.kind else {
                continue;
            };
            let TokenKind::Atom(suffix) = &instruction.tokens[index + 2].kind else {
                continue;
            };
            let candidate = format!("{base}.{suffix}");
            if let Some(operation) = operations.iter().position(|name| *name == candidate) {
                if found.replace(operation as u8).is_some() {
                    return None;
                }
            }
        }
    }
    found
}

fn subgroup_cluster(instruction: &TextInstruction) -> Option<u64> {
    for pair in instruction.tokens.windows(2) {
        if matches!(&pair[0].kind, TokenKind::Atom(name) if name == "cluster")
            && let TokenKind::Atom(value) = &pair[1].kind
        {
            return tir_format::numeric_literals::parse_unsigned_integer(value)
                .and_then(|value| u64::try_from(value).ok());
        }
    }
    None
}

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Type> {
    match instruction.opcode.as_str() {
        "gpu.builtin" => builtin_type(instruction).into_iter().collect(),
        "gpu.spec_const" => specialization_type(source, instruction)
            .into_iter()
            .collect(),
        "gpu.call.vft" => crate::gpu_vft_types::infer_result_types(operands),
        "gpu.ddx"
        | "gpu.ddy"
        | "gpu.fwidth"
        | "quad.broadcast"
        | "quad.swap"
        | "subgroup.broadcast"
        | "subgroup.broadcast_first"
        | "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate"
        | "subgroup.reduce"
        | "subgroup.scan_inclusive"
        | "subgroup.scan_exclusive" => operands.first().cloned().into_iter().collect(),
        "quad.all"
        | "quad.any"
        | "subgroup.elect"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.inverse_ballot" => vec![Type::Integer(1)],
        "subgroup.ballot" => vec![ballot_type()],
        "subgroup.ballot_bit_count" | "subgroup.ballot_find_lsb" | "subgroup.ballot_find_msb" => {
            vec![Type::Integer(32)]
        }
        _ => {
            let texture_result =
                crate::gpu_texture_types::infer_result_types(instruction, operands);
            if !texture_result.is_empty() {
                texture_result
            } else {
                let ray_result = crate::gpu_ray_types::infer_result_types(instruction);
                if ray_result.is_empty() {
                    crate::gpu_advanced_types::infer_result_types(source, instruction, operands)
                } else {
                    ray_result
                }
            }
        }
    }
}

fn verify_builtin(source: &str, instruction: &TextInstruction, diagnostics: &mut Vec<Diagnostic>) {
    let Some(expected) = builtin_type(instruction) else {
        error(diagnostics, instruction, "unknown GPU builtin name");
        return;
    };
    if let Some(actual) = builtin_declared_type(source, instruction)
        && actual != expected
    {
        error(
            diagnostics,
            instruction,
            format!("GPU builtin has type {expected}; declared type is {actual}"),
        );
    }
}

fn verify_specialization_constant(
    source: &str,
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let commas = top_level_commas(&instruction.tokens, opcode_end(instruction));
    let (Some(first), Some(second)) = (commas.first(), commas.get(1)) else {
        error(
            diagnostics,
            instruction,
            "gpu.spec_const requires an id, type, and default value",
        );
        return;
    };
    let id = instruction.tokens.get(opcode_end(instruction));
    if id
        .and_then(atom)
        .and_then(tir_format::numeric_literals::parse_unsigned_integer)
        .and_then(|value| u32::try_from(value).ok())
        .is_none()
    {
        error(
            diagnostics,
            instruction,
            "gpu.spec_const id must be a nonnegative integer",
        );
    }
    let Some(ty) = type_from_tokens(source, &instruction.tokens, *first + 1, *second) else {
        error(
            diagnostics,
            instruction,
            "gpu.spec_const requires a valid value type",
        );
        return;
    };
    if !specialization_type_supported(&ty) {
        error(
            diagnostics,
            instruction,
            "gpu.spec_const requires an integer or floating-point scalar, or a fixed vector of numeric lanes",
        );
        return;
    }
    let default_tokens: Vec<_> = instruction.tokens[*second + 1..]
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .cloned()
        .collect();
    if default_tokens.is_empty()
        || matches!(
            default_tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
    {
        error(
            diagnostics,
            instruction,
            "gpu.spec_const default must be a constant value",
        );
        return;
    }

    match &ty {
        Type::FixedVector { element, .. } => {
            diagnostics.extend(verify_literal_as_type(
                instruction,
                "const.vec",
                &ty,
                &default_tokens,
                source,
            ));
            let Some(elements) = vector_elements(&default_tokens) else {
                return;
            };
            for value in elements {
                let scalar_opcode = scalar_constant_opcode(element, &value);
                diagnostics.extend(verify_literal_as_type(
                    instruction,
                    scalar_opcode,
                    element,
                    &value,
                    source,
                ));
            }
        }
        _ => {
            let opcode = scalar_constant_opcode(&ty, &default_tokens);
            diagnostics.extend(verify_literal_as_type(
                instruction,
                opcode,
                &ty,
                &default_tokens,
                source,
            ));
        }
    }
}

fn specialization_type_supported(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => *bits <= 64,
        Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64) => true,
        Type::FixedVector { lanes, element } if matches!(*lanes, 2 | 3 | 4) => {
            matches!(element.as_ref(), Type::Integer(bits) if *bits <= 64)
                || matches!(
                    element.as_ref(),
                    Type::Float(
                        FloatType::F16
                            | FloatType::Bf16
                            | FloatType::F32
                            | FloatType::F64
                            | FloatType::F8E4M3
                            | FloatType::F8E5M2
                            | FloatType::F6E2M3
                            | FloatType::F6E3M2
                            | FloatType::F4E2M1
                            | FloatType::E8M0
                    )
                )
        }
        _ => false,
    }
}

fn scalar_constant_opcode(ty: &Type, value: &[Token]) -> &'static str {
    if matches!(ty, Type::Integer(1))
        && matches!(value.first().and_then(atom), Some("true" | "false"))
    {
        "const.bool"
    } else if matches!(ty, Type::Float(_)) {
        "const.float"
    } else {
        "const.int"
    }
}

fn verify_literal_as_type(
    original: &TextInstruction,
    opcode: &str,
    ty: &Type,
    value_tokens: &[Token],
    source: &str,
) -> Vec<Diagnostic> {
    let Some(value) = token_text(source, value_tokens) else {
        return vec![
            Diagnostic::error(VFY3, "gpu.spec_const has an invalid default value")
                .with_primary_span(original.span),
        ];
    };
    let text = if opcode == "const.bool" {
        format!("{opcode} {value}")
    } else {
        format!("{opcode} {ty}, {value}")
    };
    let Ok(tokens) = tir_format::text::lex(&text) else {
        return vec![
            Diagnostic::error(VFY3, "gpu.spec_const default is not a valid literal")
                .with_primary_span(original.span),
        ];
    };
    let synthetic = TextInstruction {
        results: Vec::new(),
        opcode: opcode.to_owned(),
        tokens,
        span: original.span,
    };
    crate::constant_types::verify(&text, &synthetic)
}

fn token_text(source: &str, tokens: &[Token]) -> Option<String> {
    let first = tokens
        .iter()
        .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    let last = tokens
        .iter()
        .rev()
        .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    source
        .get(first.span.start..last.span.end)
        .map(|text| text.to_owned())
}

fn vector_elements(tokens: &[Token]) -> Option<Vec<Vec<Token>>> {
    let open = tokens.iter().position(|token| is_punctuation(token, '['))?;
    let mut depth = 0i32;
    let mut element_start = open + 1;
    let mut elements = Vec::new();
    let mut close = None;
    for index in open..tokens.len() {
        let token = &tokens[index];
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '[' | '(' | '{' | '<' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        let element = &tokens[element_start..index];
                        if !element.is_empty() {
                            elements.push(element.to_vec());
                        }
                        close = Some(index);
                        break;
                    }
                }
                ')' | '}' | '>' => depth -= 1,
                ',' if depth == 1 => {
                    if element_start == index {
                        return None;
                    }
                    elements.push(tokens[element_start..index].to_vec());
                    element_start = index + 1;
                }
                _ => {}
            }
        }
    }
    let close = close?;
    if element_start == close && !elements.is_empty() {
        return None;
    }
    if tokens[close + 1..]
        .iter()
        .any(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
    {
        return None;
    }
    Some(elements)
}

fn builtin_type(instruction: &TextInstruction) -> Option<Type> {
    let name = atom(instruction.tokens.get(opcode_end(instruction))?)?;
    let i1 = Type::Integer(1);
    let i32 = Type::Integer(32);
    let f32 = Type::Float(FloatType::F32);
    Some(match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "threadgroups_per_grid" => vector(3, i32),
        "thread_index_in_threadgroup"
        | "subgroup_index_in_threadgroup"
        | "subgroups_per_threadgroup"
        | "lane_index"
        | "subgroup_size"
        | "vertex_id"
        | "instance_id"
        | "base_vertex"
        | "base_instance"
        | "draw_index"
        | "view_index"
        | "sample_index"
        | "sample_mask_in"
        | "primitive_id"
        | "layer"
        | "viewport_index" => i32,
        "position" => vector(4, f32),
        "front_facing" | "helper_invocation" => i1,
        "barycentric_perspective" | "barycentric_linear" => vector(3, f32),
        "point_coord" => vector(2, f32),
        "shading_rate" => vector(2, i32),
        _ => return None,
    })
}

fn builtin_declared_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = opcode_end(instruction);
    let colon = (start..tokens.len()).find(|index| is_punctuation(&tokens[*index], ':'))?;
    type_from_tokens(source, tokens, colon + 1, tokens.len())
}

fn specialization_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = opcode_end(instruction);
    let commas = top_level_commas(tokens, start);
    let (&first, &second) = (commas.first()?, commas.get(1)?);
    type_from_tokens(source, tokens, first + 1, second)
}

fn top_level_commas(tokens: &[Token], start: usize) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => commas.push(index),
                _ => {}
            }
        }
    }
    commas
}

fn type_from_tokens(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    parse_tvisa_type(source.get(first.span.start..last.span.end)?).ok()
}

fn is_float_value(ty: &Type) -> bool {
    match ty {
        Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64) => true,
        Type::FixedVector { element, .. } => is_float_value(element),
        _ => false,
    }
}

fn is_scalar_value(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_) | Type::Float(_))
}

fn ballot_type() -> Type {
    vector(4, Type::Integer(32))
}

fn vector(lanes: u32, element: Type) -> Type {
    Type::FixedVector {
        lanes,
        element: Box::new(element),
    }
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}
