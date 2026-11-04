use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextFunctionSignature, parse_type};
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::model::{Instruction, ValueDefinition, ValueUse};
use crate::ssa::{text_instruction_operands_start, text_value_uses};

mod address;
mod aggregate;
mod arity;
mod const_bytes;
mod heap;
mod loadable;
mod memory;
mod ownership;
use address::{
    infer_element_address, infer_enum_payload_address, infer_field_address, type_after_keyword,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_function(
    source: &str,
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    call_conventions: &crate::signature_types::CallConventions,
    gpu_scope: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut value_types = HashMap::new();
    let mut borrowed_values = aggregate::borrowed_parameters(body);
    for block in &body.blocks {
        for parameter in &block.parameters {
            value_types
                .entry(parameter.name.clone())
                .or_insert_with(|| parameter.ty.clone());
        }
    }

    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode == "const.bytes" {
                diagnostics.extend(const_bytes::verify(source, instruction, layouts, policies));
            }
            if matches!(instruction.opcode.as_str(), "return" | "fail") {
                diagnostics.extend(verify_exit(instruction, signature, &value_types));
                continue;
            }
            if let Some(expected) = arity::expected_operand_count(instruction.opcode.as_str()) {
                let actual = instruction_operand_count(instruction);
                if actual != expected {
                    diagnostics.push(
                        Diagnostic::error(
                            V2,
                            format!(
                                "{} has {actual} SSA operands; expected {expected}",
                                instruction.opcode
                            ),
                        )
                        .with_primary_span(instruction.span),
                    );
                    continue;
                }
            }
            let Some(operand_types) = operand_types(instruction, &value_types) else {
                continue;
            };
            diagnostics.extend(address::verify_instruction(
                source,
                instruction,
                &operand_types,
                layouts,
            ));
            let gpu_operand_types = operand_types
                .iter()
                .map(|operand| operand.ty.clone())
                .collect::<Vec<_>>();
            diagnostics.extend(crate::gpu_profile::instructions::verify(
                source,
                instruction,
                &gpu_operand_types,
            ));
            let inferred_result_types =
                infer_result_types(source, instruction, &operand_types, layouts, policies).or_else(
                    || call_conventions.inferred_result_types(instruction, source, &value_types),
                );
            diagnostics.extend(memory::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                layouts,
                policies,
            ));
            diagnostics.extend(heap::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                policies,
            ));
            diagnostics.extend(aggregate::verify_instruction(
                source,
                instruction,
                &operand_types,
                inferred_result_types.as_deref(),
                &borrowed_values,
                layouts,
                policies,
            ));
            if instruction.opcode.starts_with("vec.") {
                diagnostics.extend(crate::vector_text::verify(
                    source,
                    instruction,
                    &operand_types,
                    inferred_result_types.as_deref(),
                    instruction.results.len(),
                ));
            }
            let Some(result_types) = inferred_result_types else {
                continue;
            };
            let typed = Instruction {
                opcode: instruction.opcode.clone(),
                operands: operand_types,
                results: instruction
                    .results
                    .iter()
                    .enumerate()
                    .map(|(index, name)| ValueDefinition {
                        name: name.clone(),
                        ty: result_types
                            .get(index)
                            .or_else(|| result_types.last())
                            .cloned()
                            .unwrap_or(Type::Unit),
                        span: instruction.span,
                    })
                    .collect(),
                span: instruction.span,
            };
            diagnostics.extend(crate::instruction_types::verify_instruction(&typed));
            borrowed_values.extend(aggregate::borrowed_results(
                instruction,
                &result_types,
                &borrowed_values,
                policies,
            ));
            for (name, ty) in instruction.results.iter().zip(result_types) {
                value_types.insert(name.clone(), ty);
            }
        }
    }
    diagnostics.extend(crate::atomic_order::verify_function(
        body,
        gpu_scope,
        &value_types,
        source,
        layouts,
        policies,
    ));
    diagnostics.extend(ownership::verify(
        body,
        &borrowed_values,
        &value_types,
        policies,
        source,
        call_conventions,
    ));
    diagnostics.extend(loadable::verify(body, &value_types, layouts, policies));
    diagnostics
}

pub(crate) fn inferred_value_types(
    source: &str,
    body: &FunctionBody,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
    call_conventions: Option<&crate::signature_types::CallConventions>,
) -> HashMap<String, Type> {
    let mut value_types = HashMap::new();
    for parameter in body.blocks.iter().flat_map(|block| &block.parameters) {
        value_types
            .entry(parameter.name.clone())
            .or_insert_with(|| parameter.ty.clone());
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let uses = text_value_uses(instruction);
        let result_types = match instruction.opcode.as_str() {
            "stack.alloc" => stack_alloc_result_type(source, instruction).map(|ty| vec![ty]),
            "atomic.load" | "raw.load" | "mmio.load" => {
                explicit_type(source, instruction).map(|ty| vec![ty])
            }
            "atomic.rmw" => uses
                .last()
                .and_then(|(name, _)| value_types.get(name))
                .cloned()
                .map(|ty| vec![ty]),
            "atomic.cmpxchg" => uses
                .last()
                .and_then(|(name, _)| value_types.get(name))
                .cloned()
                .map(|ty| vec![ty, Type::Integer(1)]),
            "load" | "load.borrow" => uses
                .first()
                .and_then(|(name, _)| value_types.get(name))
                .and_then(|ty| match ty {
                    Type::Address { pointee, .. } => Some(pointee.as_ref().clone()),
                    _ => None,
                })
                .map(|ty| vec![ty]),
            _ => {
                let operands = uses
                    .iter()
                    .map(|(name, span)| {
                        value_types.get(name).cloned().map(|ty| ValueUse {
                            name: name.clone(),
                            ty,
                            span: *span,
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                operands
                    .and_then(|operands| {
                        infer_result_types(source, instruction, &operands, layouts, policies)
                    })
                    .or_else(|| {
                        call_conventions.and_then(|conventions| {
                            conventions.inferred_result_types(instruction, source, &value_types)
                        })
                    })
            }
        };
        if let Some(result_types) = result_types {
            for (index, name) in instruction.results.iter().enumerate() {
                if let Some(ty) = result_types.get(index).or_else(|| result_types.last()) {
                    value_types.insert(name.clone(), ty.clone());
                }
            }
        }
    }
    value_types
}

fn stack_alloc_result_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> Option<Type> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let first = instruction.tokens.get(start)?;
    let mut allocated = None;
    for token in instruction.tokens.iter().skip(start + 1) {
        if matches!(token.kind, TokenKind::Newline | TokenKind::End) {
            break;
        }
        let text = source.get(first.span.start..token.span.end)?;
        if let Ok(ty) = parse_type(text.trim()) {
            allocated = Some(ty);
        }
    }
    let space = instruction
        .tokens
        .windows(3)
        .find_map(|window| {
            (punctuation(&window[0], '<') && punctuation(&window[2], '>'))
                .then(|| atom_at(window, 1).and_then(address_space))
                .flatten()
        })
        .unwrap_or_else(|| default_stack_alloc_space(source));
    Some(Type::Address {
        pointee: Box::new(allocated?),
        space: Some(space),
    })
}

fn default_stack_alloc_space(source: &str) -> AddressSpace {
    let Ok(tokens) = tir_format::text::lex(source) else {
        return AddressSpace::Generic;
    };
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    if matches!(
        header.first().and_then(|token| match &token.kind {
            TokenKind::Atom(name) => Some(name.as_str()),
            _ => None,
        }),
        Some("entry")
    ) {
        return AddressSpace::Thread;
    }
    let mut in_target = false;
    let mut targets_gpu = false;
    let mut targets_cpu = false;
    for token in header {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => in_target = true,
            TokenKind::Punctuation(')') => in_target = false,
            TokenKind::Atom(name) if in_target && name == "gpu" => targets_gpu = true,
            TokenKind::Atom(name) if in_target && name == "cpu" => targets_cpu = true,
            _ => {}
        }
    }
    if targets_gpu && !targets_cpu {
        AddressSpace::Thread
    } else {
        AddressSpace::Generic
    }
}

fn address_space(name: &str) -> Option<AddressSpace> {
    Some(match name {
        "generic" => AddressSpace::Generic,
        "device" => AddressSpace::Device,
        "constant" => AddressSpace::Constant,
        "threadgroup" => AddressSpace::Threadgroup,
        "thread" => AddressSpace::Thread,
        "object_payload" => AddressSpace::ObjectPayload,
        "ray_payload" => AddressSpace::RayPayload,
        "hit_attribute" => AddressSpace::HitAttribute,
        "tile" => AddressSpace::Tile,
        "node_record" => AddressSpace::NodeRecord,
        _ => return None,
    })
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    match &tokens.get(index)?.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn verify_exit(
    instruction: &tir_format::TextInstruction,
    signature: Option<&TextFunctionSignature>,
    value_types: &HashMap<String, Type>,
) -> Vec<Diagnostic> {
    let Some(signature) = signature else {
        return Vec::new();
    };
    let uses = text_value_uses(instruction);
    let expected = if instruction.opcode == "return" {
        Some(&signature.result)
    } else {
        signature.failure.as_ref()
    };
    let mut diagnostics = Vec::new();
    let Some(expected) = expected else {
        diagnostics.push(
            Diagnostic::error(
                V2,
                if instruction.opcode == "fail" {
                    "fail appears in a function without a failure type"
                } else {
                    "function result type is missing"
                },
            )
            .with_primary_span(instruction.span),
        );
        return diagnostics;
    };
    if instruction.opcode == "return" && matches!(expected, Type::Never) {
        diagnostics.push(
            Diagnostic::error(V2, "return appears in a never-result function")
                .with_primary_span(instruction.span),
        );
        return diagnostics;
    }
    let carries_value = !matches!(expected, Type::Unit);
    let expected_count = if carries_value { 1 } else { 0 };
    if uses.len() != expected_count {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} carries {} values; function signature requires {expected_count}",
                    instruction.opcode,
                    uses.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
        return diagnostics;
    }
    if let Some((name, span)) = uses.first()
        && let Some(actual) = value_types.get(name)
        && actual != expected
    {
        diagnostics.push(
            Diagnostic::error(
                V2,
                format!(
                    "{} value %{name} has type {actual}; function signature requires {expected}",
                    instruction.opcode
                ),
            )
            .with_primary_span(*span),
        );
    }
    diagnostics
}

fn instruction_operand_count(instruction: &tir_format::TextInstruction) -> usize {
    if is_conversion(instruction.opcode.as_str()) {
        let start = text_instruction_operands_start(&instruction.tokens);
        return usize::from(
            instruction.tokens[start..]
                .windows(2)
                .any(|window| matches!(window[0].kind, TokenKind::Punctuation('%'))),
        );
    }
    text_value_uses(instruction).len()
}

fn operand_types(
    instruction: &tir_format::TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Option<Vec<ValueUse>> {
    if is_operandless_definition(instruction.opcode.as_str()) {
        return Some(Vec::new());
    }
    if is_conversion(instruction.opcode.as_str()) {
        let start = text_instruction_operands_start(&instruction.tokens);
        let (name, span) = instruction.tokens[start..].windows(2).find_map(|window| {
            match (&window[0].kind, &window[1].kind) {
                (TokenKind::Punctuation('%'), TokenKind::Atom(name)) => {
                    Some((name.clone(), window[1].span))
                }
                _ => None,
            }
        })?;
        return Some(vec![ValueUse {
            ty: value_types.get(&name)?.clone(),
            name,
            span,
        }]);
    }
    text_value_uses(instruction)
        .into_iter()
        .map(|(name, span)| {
            value_types
                .get(&name)
                .cloned()
                .map(|ty| ValueUse { name, ty, span })
        })
        .collect()
}

fn infer_result_types(
    source: &str,
    instruction: &tir_format::TextInstruction,
    operands: &[ValueUse],
    layouts: &LayoutEngine,
    policies: &TypePolicies,
) -> Option<Vec<Type>> {
    if aggregate::handles(instruction.opcode.as_str()) {
        return aggregate::infer_result_types(source, instruction, operands, layouts, policies);
    }
    let gpu_operand_types = operands
        .iter()
        .map(|operand| operand.ty.clone())
        .collect::<Vec<_>>();
    if let Some(result_types) = crate::gpu_profile::instructions::infer_result_types(
        source,
        instruction,
        &gpu_operand_types,
    ) {
        return Some(result_types);
    }
    let opcode = instruction.opcode.as_str();
    let first = || operands.first().map(|operand| operand.ty.clone());
    if opcode.starts_with("vec.") {
        return crate::vector_text::infer_result_types(source, instruction, operands);
    }
    match opcode {
        "const.int" | "const.float" | "const.zero" | "const.bytes" | "const.aggregate"
        | "const.vec" | "const.poison" => Some(vec![explicit_type(source, instruction)?]),
        "trunc" | "zext" | "sext" | "fptrunc" | "fpext" | "sitofp" | "uitofp" | "fptosi"
        | "fptoui" | "ptrtoint" | "inttoptr" | "bitcast" | "fcvt" => {
            Some(vec![target_type(source, instruction)?])
        }
        "field.addr" => Some(vec![infer_field_address(instruction, &first()?, layouts)?]),
        "elem.addr" => Some(vec![infer_element_address(&first()?)?]),
        "enum.payload.addr" => Some(vec![infer_enum_payload_address(
            instruction,
            &first()?,
            layouts,
        )?]),
        "addr.cast" => Some(vec![type_after_keyword(source, instruction, "to")?]),
        "addr.toptr" => {
            let Type::Address { space, .. } = first()? else {
                return None;
            };
            Some(vec![Type::Pointer(space.unwrap_or(AddressSpace::Generic))])
        }
        "ptr.toaddr" => Some(vec![type_after_keyword(source, instruction, "as")?]),
        "const.bool" => Some(vec![Type::Integer(1)]),
        "const.null" => Some(vec![Type::Pointer(null_space(instruction))]),
        "global.addr" | "tls.addr" | "descriptor.ref" | "witness.ref" | "block.addr" => {
            Some(vec![Type::Pointer(AddressSpace::Generic)])
        }
        "const.sampler" => Some(vec![Type::Sampler]),
        "const.string" => Some(vec![Type::Tuple(vec![
            Type::Pointer(AddressSpace::Generic),
            Type::Integer(64),
        ])]),
        "add.ovf" | "sub.ovf" | "mul.ovf" => {
            let ty = first()?;
            Some(vec![Type::Tuple(vec![ty.clone(), mask_type(&ty)])])
        }
        "icmp" | "fcmp" => Some(vec![mask_type(&first()?)]),
        "select" => operands.get(1).map(|operand| vec![operand.ty.clone()]),
        "fclass" => Some(vec![class_type(&first()?)]),
        "frexp" => {
            let ty = first()?;
            Some(vec![Type::Tuple(vec![ty.clone(), integer_shape(&ty, 32)])])
        }
        "load" | "load.borrow" => first().and_then(|ty| match ty {
            Type::Address { pointee, .. } => Some(vec![pointee.as_ref().clone()]),
            _ => None,
        }),
        "heap.alloc.counted" | "heap.alloc.unique" | "ref.addr" | "uniq.addr"
        | "heap.alloc.raw" | "heap.realloc.raw" | "weak.new" | "weak.load" | "ref.unowned"
        | "ref.isunique" => heap::infer_result_types(source, instruction, &operands, policies),
        "borrow.begin" => first().map(|ty| vec![ty]),
        "atomic.load" => Some(vec![explicit_type(source, instruction)?]),
        "atomic.rmw" => operands.get(1).map(|operand| vec![operand.ty.clone()]),
        "atomic.cmpxchg" => operands
            .get(1)
            .map(|operand| vec![operand.ty.clone(), Type::Integer(1)]),
        "atomic.wait" | "atomic.notify" => Some(vec![Type::Integer(32)]),
        "fp.flags.read" => Some(vec![Type::Integer(32)]),
        "fp.flags.clear" => Some(vec![Type::Unit]),
        "br" | "cond.br" | "switch.int" | "switch.enum" | "br.indirect" | "return" | "fail"
        | "tail.call" | "unreachable" | "trap" | "try.call" | "invoke" | "coro.yield"
        | "coro.resume" | "coro.finish" | "resume" | "throw" | "longjmp" | "asm.goto" | "deopt"
        | "rt.ignore_hit" | "rt.accept_and_end" => None,
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "srem" | "urem" | "and" | "or" | "xor"
        | "shl" | "lshr" | "ashr" | "rotl" | "rotr" | "smin" | "smax" | "umin" | "umax"
        | "mulh" | "clmul" | "clmulh" | "fshl" | "fshr" | "not" | "neg" | "abs" | "ctlz"
        | "cttz" | "ctpop" | "bswap" | "bitreverse" | "freeze" | "fneg" | "fabs" | "fsqrt"
        | "ffloor" | "fceil" | "ftrunc" | "fround" | "froundeven" | "fadd" | "fsub" | "fmul"
        | "fdiv" | "frem" | "fmin" | "fmax" | "fminnum" | "fmaxnum" | "fcopysign" | "fma"
        | "fmuladd" | "math.atan2" | "math.pow" | "math.hypot" | "math.sin" | "math.cos"
        | "math.tan" | "math.asin" | "math.acos" | "math.atan" | "math.sinh" | "math.cosh"
        | "math.tanh" | "math.asinh" | "math.acosh" | "math.atanh" | "math.exp" | "math.exp2"
        | "math.exp10" | "math.expm1" | "math.log" | "math.log2" | "math.log10" | "math.log1p"
        | "math.cbrt" | "math.rsqrt" => first().map(|ty| vec![ty]),
        "ldexp" => first().map(|ty| vec![ty]),
        _ => None,
    }
}

pub(crate) fn explicit_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> Option<Type> {
    let mut start_index = text_instruction_operands_start(&instruction.tokens);
    if instruction.opcode == "atomic.load" {
        while matches!(
            instruction.tokens.get(start_index).map(|token| &token.kind),
            Some(TokenKind::Punctuation('['))
        ) {
            start_index = instruction.tokens[start_index..]
                .iter()
                .position(|token| matches!(token.kind, TokenKind::Punctuation(']')))?
                .checked_add(start_index + 1)?;
        }
    }
    let start_token = instruction.tokens.get(start_index)?;
    let start = start_token.span.start;
    let mut nesting = 0usize;
    let mut end = instruction.span.end;
    for token in &instruction.tokens[start_index..] {
        match token.kind {
            TokenKind::Punctuation('<' | '(' | '[' | '{') => nesting += 1,
            TokenKind::Punctuation('>' | ')' | ']' | '}') => nesting = nesting.saturating_sub(1),
            TokenKind::Punctuation(',') if nesting == 0 => {
                end = token.span.start;
                break;
            }
            TokenKind::Punctuation('!') if nesting == 0 => {
                end = token.span.start;
                break;
            }
            _ => {}
        }
    }
    let text = source.get(start..end)?.trim();
    parse_type(text).ok()
}

fn target_type(source: &str, instruction: &tir_format::TextInstruction) -> Option<Type> {
    type_after_keyword(source, instruction, "to")
}

fn null_space(instruction: &tir_format::TextInstruction) -> AddressSpace {
    let mut start = text_instruction_operands_start(&instruction.tokens);
    if matches!(
        instruction.tokens.get(start).map(|token| &token.kind),
        Some(TokenKind::Punctuation('['))
    ) {
        start += 1;
    }
    let name = instruction
        .tokens
        .get(start)
        .and_then(|token| match &token.kind {
            TokenKind::Atom(value) => Some(value.as_str()),
            _ => None,
        });
    match name {
        Some("device") => AddressSpace::Device,
        Some("constant") => AddressSpace::Constant,
        Some("threadgroup") => AddressSpace::Threadgroup,
        Some("thread") => AddressSpace::Thread,
        Some("object_payload") => AddressSpace::ObjectPayload,
        Some("ray_payload") => AddressSpace::RayPayload,
        Some("hit_attribute") => AddressSpace::HitAttribute,
        Some("tile") => AddressSpace::Tile,
        Some("node_record") => AddressSpace::NodeRecord,
        _ => AddressSpace::Generic,
    }
}

fn is_operandless_definition(opcode: &str) -> bool {
    opcode.starts_with("const.")
        || matches!(
            opcode,
            "global.addr"
                | "tls.addr"
                | "descriptor.ref"
                | "witness.ref"
                | "block.addr"
                | "const.sampler"
        )
}

fn is_conversion(opcode: &str) -> bool {
    matches!(
        opcode,
        "trunc"
            | "zext"
            | "sext"
            | "fptrunc"
            | "fpext"
            | "sitofp"
            | "uitofp"
            | "fptosi"
            | "fptoui"
            | "ptrtoint"
            | "inttoptr"
            | "bitcast"
            | "fcvt"
    )
}

fn mask_type(ty: &Type) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(1)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(1))),
        _ => Type::Integer(1),
    }
}

fn class_type(ty: &Type) -> Type {
    integer_shape(ty, 32)
}

fn integer_shape(ty: &Type, width: u16) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(width)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(width))),
        _ => Type::Integer(width),
    }
}
