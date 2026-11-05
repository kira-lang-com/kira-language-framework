use super::*;

pub(super) fn instruction_operand_count(instruction: &tir_format::TextInstruction) -> usize {
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

pub(super) fn operand_types(
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

pub(super) fn infer_result_types(
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
        "closure.new" => match first()? {
            Type::Function(signature) => Some(vec![Type::Closure(ClosureType {
                borrowed: false,
                signature,
            })]),
            _ => None,
        },
        "closure.callback" => closure_callback_type(source, instruction)
            .map(|(_, signature)| vec![Type::Function(signature)]),
        "call.closure" => closure_call_result_type(&first()?).map(|ty| vec![ty]),
        "call.indirect" => crate::signature_types::indirect_call_type(source, instruction)
            .and_then(|(signature, _)| synchronous_call_result_type(&signature))
            .map(|ty| vec![ty]),
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

pub(super) fn closure_call_result_type(ty: &Type) -> Option<Type> {
    match ty {
        Type::Closure(closure) => synchronous_call_result_type(&closure.signature),
        _ => None,
    }
}

pub(super) fn direct_call_result_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
    call_conventions: &crate::signature_types::CallConventions,
) -> Option<Type> {
    if matches!(instruction.opcode.as_str(), "call.witness" | "call.vtable") {
        let result = call_conventions.special_result(instruction, source)?;
        if result.asynchronous || matches!(&result.result, Type::Generic(_)) {
            return None;
        }
        return Some(result.result);
    }
    if !matches!(
        instruction.opcode.as_str(),
        "call" | "call.foreign" | "call.variadic"
    ) {
        return None;
    }
    if instruction.opcode == "call"
        && let Some(Ok(signature)) = call_conventions.specialized_direct_call(source, instruction)
    {
        if signature.asynchronous {
            let (symbol, _, _) =
                instruction
                    .tokens
                    .iter()
                    .enumerate()
                    .find_map(|(index, token)| {
                        matches!(token.kind, TokenKind::Punctuation('@'))
                            .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                            .flatten()
                    })?;
            let frame = if symbol.contains('.') || call_conventions.module_path().is_empty() {
                symbol
            } else {
                format!("{}.{}", call_conventions.module_path(), symbol)
            };
            return Some(Type::Frame(frame));
        }
        return (!matches!(signature.result.as_ref(), Type::Generic(_)))
            .then_some(signature.result.as_ref().clone());
    }
    let (symbol, _, _) = instruction
        .tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                .flatten()
        })?;
    let result = call_conventions.direct_result(&symbol)?;
    if result.asynchronous {
        let frame = if symbol.contains('.') || call_conventions.module_path().is_empty() {
            symbol
        } else {
            format!("{}.{}", call_conventions.module_path(), symbol)
        };
        Some(Type::Frame(frame))
    } else if matches!(&result.result, Type::Generic(_)) {
        None
    } else {
        Some(result.result)
    }
}

pub(super) fn synchronous_call_result_type(signature: &FunctionType) -> Option<Type> {
    (!signature.asynchronous).then(|| signature.result.as_ref().clone())
}

pub(super) fn closure_callback_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> Option<(usize, FunctionType)> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let parameter_index = match &tokens.get(start)?.kind {
        TokenKind::Atom(value) => value.parse().ok()?,
        _ => return None,
    };
    let comma = tokens
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation(',')).then_some(index)
        })?;
    let type_start = tokens.get(comma + 1)?.span.start;
    let mut nesting = 0usize;
    let mut type_end = instruction.span.end;
    for token in &tokens[comma + 1..] {
        match token.kind {
            TokenKind::Punctuation('(' | '[' | '<' | '{') => nesting += 1,
            TokenKind::Punctuation(')' | ']' | '>' | '}') => {
                nesting = nesting.saturating_sub(1);
            }
            TokenKind::Punctuation('!') if nesting == 0 => {
                type_end = token.span.start;
                break;
            }
            TokenKind::Newline | TokenKind::End => {
                type_end = token.span.start;
                break;
            }
            _ => {}
        }
    }
    let Type::Function(signature) = parse_type(source.get(type_start..type_end)?.trim()).ok()?
    else {
        return None;
    };
    Some((parameter_index, signature))
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

pub(super) fn target_type(source: &str, instruction: &tir_format::TextInstruction) -> Option<Type> {
    type_after_keyword(source, instruction, "to")
}

pub(super) fn null_space(instruction: &tir_format::TextInstruction) -> AddressSpace {
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

pub(super) fn is_operandless_definition(opcode: &str) -> bool {
    opcode.starts_with("const.")
        || matches!(
            opcode,
            "func.ref"
                | "global.addr"
                | "tls.addr"
                | "descriptor.ref"
                | "witness.ref"
                | "block.addr"
                | "foreign.addr"
                | "foreign.data"
                | "foreign.entry"
                | "kernel.percpu.addr"
                | "const.sampler"
        )
}

pub(super) fn is_conversion(opcode: &str) -> bool {
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

pub(super) fn mask_type(ty: &Type) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(1)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(1))),
        _ => Type::Integer(1),
    }
}

pub(super) fn class_type(ty: &Type) -> Type {
    integer_shape(ty, 32)
}

pub(super) fn integer_shape(ty: &Type, width: u16) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(width)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(width))),
        _ => Type::Integer(width),
    }
}
