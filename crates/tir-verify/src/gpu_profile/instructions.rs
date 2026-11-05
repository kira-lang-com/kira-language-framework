use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_type};
use tir_types::{FloatType, Type};

use crate::ssa::text_instruction_operands_start;

mod specialization;

use specialization::{specialization_constant_default_is_valid, specialization_constant_type};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if looks_like_gpu_opcode(&instruction.opcode) && !is_gpu_opcode(&instruction.opcode) {
        error(
            &mut diagnostics,
            instruction,
            format!("unknown GPU opcode {}", instruction.opcode),
        );
        return diagnostics;
    }
    match instruction.opcode.as_str() {
        "gpu.builtin" => {
            if instruction.results.len() != 1 {
                error(
                    &mut diagnostics,
                    instruction,
                    "gpu.builtin must define exactly one result",
                );
            }
            let Some(expected) = builtin_type(instruction) else {
                error(&mut diagnostics, instruction, "unknown GPU builtin name");
                return diagnostics;
            };
            match declared_builtin_type(source, instruction) {
                Some(actual) if actual != expected => error(
                    &mut diagnostics,
                    instruction,
                    format!("GPU builtin has type {expected}; declared type is {actual}"),
                ),
                None => error(
                    &mut diagnostics,
                    instruction,
                    "gpu.builtin requires an explicit result type",
                ),
                _ => {}
            }
        }
        "gpu.spec_const" => {
            if instruction.results.len() != 1 {
                error(
                    &mut diagnostics,
                    instruction,
                    "gpu.spec_const must define exactly one result",
                );
            }
            if specialization_constant_type(source, instruction).is_none() {
                error(
                    &mut diagnostics,
                    instruction,
                    "gpu.spec_const requires a nonnegative id, a valid type, and a default value",
                );
            } else if !specialization_constant_default_is_valid(source, instruction) {
                error(
                    &mut diagnostics,
                    instruction,
                    "gpu.spec_const default must match its scalar or fixed-vector type",
                );
            }
        }
        _ => {}
    }
    let result_count = super::texture_types::result_arity(instruction)
        .or_else(|| infer_result_types(source, instruction, operands).map(|types| types.len()));
    if let Some(expected) = result_count
        && expected != instruction.results.len()
    {
        error(
            &mut diagnostics,
            instruction,
            format!(
                "{} defines {} results; instruction yields {}",
                instruction.opcode,
                instruction.results.len(),
                expected
            ),
        );
    }
    diagnostics.extend(super::texture_types::verify(instruction, operands));
    diagnostics.extend(super::vft_types::verify(instruction, operands));
    diagnostics.extend(super::ray_types::verify(instruction, operands));
    diagnostics.extend(super::advanced_types::verify(source, instruction, operands));
    diagnostics.extend(super::sync::verify(instruction));
    verify_subgroup_types(instruction, operands, &mut diagnostics);
    diagnostics
}

pub(crate) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Option<Vec<Type>> {
    let opcode = instruction.opcode.as_str();
    match opcode {
        "gpu.builtin" => Some(vec![builtin_type(instruction)?]),
        "gpu.spec_const" => Some(vec![specialization_constant_type(source, instruction)?]),
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
        | "subgroup.scan_exclusive" => operands.first().cloned().map(|ty| vec![ty]),
        "quad.all"
        | "quad.any"
        | "subgroup.elect"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.inverse_ballot" => Some(vec![Type::Integer(1)]),
        "subgroup.ballot" => Some(vec![ballot_type()]),
        "subgroup.ballot_bit_count" | "subgroup.ballot_find_lsb" | "subgroup.ballot_find_msb" => {
            Some(vec![Type::Integer(32)])
        }
        "frag.load_attachment" => Some(vec![vector(4, Type::Float(FloatType::F32))]),
        opcode if opcode.starts_with("tex.") => Some(super::texture_types::infer_result_types(
            instruction,
            operands,
        )),
        "gpu.call.vft" => Some(super::vft_types::infer_result_types(operands)),
        opcode if is_ray_opcode(opcode) => Some(super::ray_types::infer_result_types(instruction)),
        opcode if is_advanced_opcode(opcode) => Some(super::advanced_types::infer_result_types(
            source,
            instruction,
            operands,
        )),
        opcode if is_gpu_opcode(opcode) => Some(Vec::new()),
        _ => None,
    }
}

pub(crate) fn expected_operand_count(opcode: &str) -> Option<usize> {
    Some(match opcode {
        "gpu.builtin" | "gpu.spec_const" | "subgroup.elect" => 0,
        "gpu.barrier"
        | "gpu.memfence"
        | "frag.demote"
        | "frag.load_attachment"
        | "frag.raster_order.begin"
        | "frag.raster_order.end"
        | "frag.imageblock.addr"
        | "rt.get"
        | "rt.hitobject.miss"
        | "node.input.count"
        | "node.finished_cross_group"
        | "coop.length" => 0,
        "gpu.ddx"
        | "gpu.ddy"
        | "gpu.fwidth"
        | "quad.swap"
        | "quad.all"
        | "quad.any"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.broadcast_first"
        | "subgroup.ballot"
        | "subgroup.inverse_ballot"
        | "subgroup.ballot_bit_count"
        | "subgroup.ballot_find_lsb"
        | "subgroup.ballot_find_msb"
        | "subgroup.reduce"
        | "subgroup.scan_inclusive"
        | "subgroup.scan_exclusive"
        | "rq.proceed"
        | "rq.commit_triangle"
        | "rq.abort"
        | "rq.get"
        | "rt.hitobject.from_query"
        | "rt.hitobject.get"
        | "tile.imageblock.addr"
        | "coop.convert"
        | "coop.reduce"
        | "tensor.query"
        | "node.input.addr"
        | "node.output.commit" => 1,
        "subgroup.broadcast"
        | "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate"
        | "quad.broadcast"
        | "mesh.set_counts"
        | "mesh.write.vertex"
        | "mesh.write.primitive"
        | "mesh.write.indices"
        | "rq.commit_procedural"
        | "rt.call_callable"
        | "rt.hitobject.invoke"
        | "coop.extract"
        | "tensor.load"
        | "node.output.addr"
        | "node.output.allocate" => 2,
        "object.dispatch_mesh"
        | "rt.report_hit"
        | "coop.store"
        | "coop.insert"
        | "tensor.store"
        | "coop.mma" => 3,
        "rq.init" => 8,
        "rt.trace" => 11,
        "coop.fill" => 1,
        "coop.load" => 2,
        _ => return None,
    })
}

fn verify_subgroup_types(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    match instruction.opcode.as_str() {
        "gpu.ddx" | "gpu.ddy" | "gpu.fwidth" => {
            if operands.first().is_some_and(|ty| !is_float_value(ty)) {
                error(
                    diagnostics,
                    instruction,
                    format!("{} requires a floating-point value", instruction.opcode),
                );
            }
        }
        "quad.all" | "quad.any" | "subgroup.all" | "subgroup.any" | "subgroup.ballot" => {
            if operands.first().is_some_and(|ty| ty != &Type::Integer(1)) {
                error(
                    diagnostics,
                    instruction,
                    format!("{} requires an i1 value", instruction.opcode),
                );
            }
        }
        "subgroup.all_equal" => {
            if operands.first().is_some_and(|ty| !is_scalar_value(ty)) {
                error(
                    diagnostics,
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
                    diagnostics,
                    instruction,
                    format!("{} requires vec<4 x i32>", instruction.opcode),
                );
            }
        }
        _ => {}
    }
}

fn is_gpu_opcode(opcode: &str) -> bool {
    matches!(
        opcode,
        "gpu.builtin"
            | "gpu.spec_const"
            | "gpu.barrier"
            | "gpu.memfence"
            | "gpu.call.vft"
            | "gpu.printf"
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
            | "subgroup.scan_exclusive"
            | "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.read"
            | "tex.write"
            | "tex.atomic"
            | "tex.query"
            | "tex.lod"
            | "tex.feedback"
            | "object.dispatch_mesh"
            | "mesh.set_counts"
            | "mesh.write.vertex"
            | "mesh.write.primitive"
            | "mesh.write.indices"
            | "rq.init"
            | "rq.proceed"
            | "rq.commit_triangle"
            | "rq.commit_procedural"
            | "rq.abort"
            | "rq.get"
            | "rt.trace"
            | "rt.report_hit"
            | "rt.ignore_hit"
            | "rt.accept_and_end"
            | "rt.call_callable"
            | "rt.get"
            | "rt.hitobject.trace"
            | "rt.hitobject.from_query"
            | "rt.hitobject.miss"
            | "rt.hitobject.invoke"
            | "rt.hitobject.get"
            | "rt.reorder"
            | "coop.fill"
            | "coop.load"
            | "coop.store"
            | "coop.mma"
            | "coop.convert"
            | "coop.length"
            | "coop.extract"
            | "coop.insert"
            | "coop.reduce"
            | "tensor.load"
            | "tensor.store"
            | "tensor.query"
            | "frag.demote"
            | "frag.load_attachment"
            | "frag.raster_order.begin"
            | "frag.raster_order.end"
            | "frag.imageblock.addr"
            | "tile.imageblock.addr"
            | "node.input.addr"
            | "node.input.count"
            | "node.output.allocate"
            | "node.output.addr"
            | "node.output.commit"
            | "node.finished_cross_group"
    )
}

fn looks_like_gpu_opcode(opcode: &str) -> bool {
    [
        "gpu.",
        "quad.",
        "subgroup.",
        "tex.",
        "frag.",
        "mesh.",
        "object.",
        "rq.",
        "rt.",
        "coop.",
        "tensor.",
        "tile.",
        "node.",
    ]
    .iter()
    .any(|prefix| opcode.starts_with(prefix))
}

fn is_ray_opcode(opcode: &str) -> bool {
    opcode.starts_with("rq.") || opcode.starts_with("rt.")
}

fn is_advanced_opcode(opcode: &str) -> bool {
    matches!(
        opcode,
        "object.dispatch_mesh"
            | "mesh.set_counts"
            | "mesh.write.vertex"
            | "mesh.write.primitive"
            | "mesh.write.indices"
            | "coop.fill"
            | "coop.load"
            | "coop.store"
            | "coop.mma"
            | "coop.convert"
            | "coop.length"
            | "coop.extract"
            | "coop.insert"
            | "coop.reduce"
            | "tensor.load"
            | "tensor.store"
            | "tensor.query"
            | "frag.imageblock.addr"
            | "tile.imageblock.addr"
            | "node.input.addr"
            | "node.input.count"
            | "node.output.allocate"
            | "node.output.addr"
            | "node.output.commit"
            | "node.finished_cross_group"
    )
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

fn builtin_type(instruction: &TextInstruction) -> Option<Type> {
    let name = atom(
        instruction
            .tokens
            .get(text_instruction_operands_start(&instruction.tokens))?,
    )?;
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

fn declared_builtin_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let colon = (start..tokens.len()).find(|index| punctuation(&tokens[*index], ':'))?;
    type_range(source, tokens, colon + 1, tokens.len())
}

fn type_range(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = (start..end).rev().find_map(|index| {
        tokens
            .get(index)
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
    })?;
    parse_type(source.get(first.span.start..last.span.end)?.trim()).ok()
}

fn top_level_commas(tokens: &[Token], start: usize) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' | '{' => depth += 1,
                '>' | ']' | ')' | '}' => depth -= 1,
                ',' if depth == 0 => commas.push(index),
                _ => {}
            }
        }
    }
    commas
}

fn vector(lanes: u32, element: Type) -> Type {
    Type::FixedVector {
        lanes,
        element: Box::new(element),
    }
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message).with_primary_span(instruction.span));
}
