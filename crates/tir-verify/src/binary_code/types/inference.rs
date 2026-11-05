use super::*;

pub(super) fn infer_value_types(
    body: &BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<Option<u64>> {
    let mut value_types = Vec::new();
    let target = usize::try_from(body.function_index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .map(|signature| signature.target);
    for block in &body.blocks {
        value_types.extend(block.parameters.iter().map(|parameter| Some(parameter.ty)));
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                value_types.extend(infer_instruction_types(
                    instruction,
                    &value_types,
                    signatures,
                    types,
                    target,
                ));
            }
        }
    }
    value_types
}

fn infer_instruction_types(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    target: Option<BinaryTarget>,
) -> Vec<Option<u64>> {
    let count = usize::try_from(instruction.result_count).unwrap_or(0);
    let mut results = vec![None; count];
    if count == 0 {
        return results;
    }
    if matches!(instruction.opcode, 0x103 | 0x106) {
        return infer_destructure_results(instruction, value_types, types, count);
    }
    let first = value_type_at(instruction, 0, value_types);
    let inferred = (|| -> Option<u64> {
        match instruction.opcode {
            0x000 | 0x001 | 0x004 | 0x005 | 0x007..=0x009 | 0x222 | 0x400 | 0x401 => {
                explicit_type(instruction)
            }
            0x002 | 0x223 | 0x425 | 0x426 | 0x440..=0x443 | 0x447 | 0x501 | 0x507 => {
                integer_type(types, 1)
            }
            0x003 => index_operand(instruction).and_then(|space| pointer_type(types, space as u8)),
            0x006 => string_type(types),
            0x00a => function_reference_type(instruction, signatures, types),
            0x00b..=0x00f => pointer_type(types, 0),
            0x011 => type_index(types, |ty| matches!(ty, BinaryType::Sampler)),
            0x020..=0x045 => match instruction.opcode - 0x020 {
                3..=5 => overflow_result(first?, types),
                28 => first.and_then(|ty| lane_type(types, ty, integer_type(types, 1)?)),
                29 => value_type_at(instruction, 1, value_types),
                _ => first,
            },
            0x060..=0x092 => match instruction.opcode - 0x060 {
                20 => first.and_then(|ty| lane_type(types, ty, integer_type(types, 1)?)),
                21 => first.and_then(|ty| lane_type(types, ty, integer_type(types, 32)?)),
                22 => {
                    let value = first?;
                    let exponent = lane_type(types, value, integer_type(types, 32)?)?;
                    tuple_type(types, &[value, exponent])
                }
                23 => first,
                24 => integer_type(types, 32),
                _ => first,
            },
            0x0a0..=0x0ac => explicit_type(instruction),
            0x0c0..=0x0d7 => infer_vector_result(instruction, first, value_types, types),
            0x100 | 0x107 | 0x10a => explicit_type(instruction),
            0x104 => infer_tuple_new(instruction, value_types, types),
            0x101 | 0x105 | 0x108 | 0x10b | 0x10c | 0x10d => {
                infer_aggregate_result(instruction, first, types)
            }
            0x102 | 0x109 => first,
            0x120 => explicit_type(instruction)
                .and_then(|pointee| stack_allocation_type(instruction, types, pointee, target)),
            0x121
            | 0x125
            | 0x142..=0x145
            | 0x147
            | 0x14a..=0x14d
            | 0x166
            | 0x182
            | 0x184
            | 0x187
            | 0x188
            | 0x201
            | 0x204
            | 0x1a7
            | 0x1aa
            | 0x1e0..=0x1e9 => None,
            0x122 => address::infer_field_address(instruction, first, types),
            0x123 => address::infer_element_address(first, types),
            0x124 => address::infer_enum_payload_address(instruction, first, types),
            0x126 | 0x128 => explicit_type(instruction),
            0x127 => first.and_then(|ty| {
                address_space(types, ty).and_then(|space| pointer_type(types, space))
            }),
            0x140 | 0x141 => first.and_then(|ty| address_pointee(types, ty)),
            0x146 | 0x270 => explicit_type(instruction),
            0x148 => first,
            0x149 => integer_type(types, 64),
            0x1d0 => {
                let pointer = pointer_type(types, 0)?;
                let tag = integer_type(types, 32)?;
                tuple_type(types, &[pointer, tag])
            }
            0x1d3 => pointer_type(types, 0),
            0x1d4 => integer_type(types, 64),
            0x160 => explicit_type(instruction).and_then(|pointee| reference_type(types, pointee)),
            0x161 => explicit_type(instruction).and_then(|pointee| unique_type(types, pointee)),
            0x162 | 0x163 => first.and_then(|ty| {
                reference_pointee(types, ty).and_then(|pointee| address_type(types, pointee, 0))
            }),
            0x164 => pointer_type(types, 0),
            0x165 => first,
            0x167 => first.and_then(|ty| {
                reference_pointee(types, ty).and_then(|pointee| weak_type(types, pointee))
            }),
            0x168 => heap::weak_load_result(first, types),
            0x169 => pointer_type(types, 0),
            0x16a => integer_type(types, 1),
            0x180 | 0x181 | 0x183 => first,
            0x185 => pointer_type(types, 0),
            0x186 => explicit_type(instruction),
            0x200 => explicit_type(instruction),
            0x205 | 0x206 => integer_type(types, 32),
            0x202 => value_type_at(instruction, 1, value_types),
            0x203 => atomic_compare_exchange_type(instruction, value_types, types),
            0x210..=0x215 => infer_vararg_result(instruction, types),
            0x250..=0x252 => integer_type(types, 64),
            0x224 => integer_type(types, 64),
            0x230 => intrinsic_result(instruction, first, value_types, types),
            0x244 => value_type_at(instruction, 0, value_types),
            0x273 | 0x281 => pointer_type(types, 0),
            0x280 | 0x282 => gpu_entry_function_type(instruction, signatures),
            0x404 => explicit_type(instruction).and_then(|ty| function_result(types, ty)),
            0x406 => vector_type(types, 4, float_type(types, FloatType::F32)?),
            0x409 | 0x5a0 => pointer_type(types, 8),
            0x420..=0x424 | 0x445 | 0x44b..=0x452 => first,
            0x446 => vector_type(types, 4, integer_type(types, 32)?),
            0x448..=0x44a => integer_type(types, 32),
            0x480..=0x48d => gpu::infer_texture_results(instruction, first, types, &mut results),
            0x4c0..=0x4c4 => None,
            0x500..=0x511 => gpu::infer_ray_results(instruction, types, &mut results),
            0x560 | 0x561 => explicit_type(instruction),
            0x563 => value_type_at(instruction, 2, value_types),
            0x564 => explicit_type(instruction),
            0x565 | 0x56b => integer_type(types, if instruction.opcode == 0x565 { 32 } else { 64 }),
            0x566 => first.and_then(|ty| matrix_element_type(types, ty)),
            0x567 | 0x568 => first,
            0x569 => first.and_then(|ty| tensor_element_type(types, ty)),
            0x5c0 | 0x5c2 | 0x5c3 => pointer_type(types, 9),
            0x5c1 => integer_type(types, 32),
            0x5c5 => integer_type(types, 1),
            0x1a0 | 0x1a8 => direct_call_result(instruction, signatures, types),
            0x1a1 | 0x1a9 => explicit_type(instruction).and_then(|ty| callable_result(types, ty)),
            0x1a3 => first.and_then(|ty| closure_result(types, ty)),
            0x1a2 => infer_closure_type(instruction, value_types, types),
            0x1a4 => instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Type(index) => Some(*index),
                    _ => None,
                }),
            0x1a5 | 0x1a6 => None,
            0x1c0 => coroutines::infer_start_result(instruction, signatures, types),
            0x1c5 => integer_type(types, 1),
            0x1c6 | 0x1c7 => integer_type(types, 64),
            0x1c8 => coroutines::infer_await_result(instruction, value_types, signatures, types),
            0x1c9 => coroutines::infer_async_waker(types),
            _ => first,
        }
    })();
    if let Some(result) = results.first_mut() {
        *result = inferred;
    }
    results
}

fn infer_vector_result(
    instruction: &BinaryInstruction,
    first: Option<u64>,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<u64> {
    let relative = instruction.opcode - 0x0c0;
    match relative {
        0 | 4 | 7 | 9 => explicit_type(instruction),
        1 | 6 => first.and_then(|ty| vector_element_type(types, ty)),
        2 | 11..=17 => first,
        18 => infer_concat_result(first, value_type_at(instruction, 1, value_types), types),
        3 => {
            let element = first.and_then(|ty| vector_element_type(types, ty))?;
            let lanes = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Integers(indices) => Some(indices.len() as u64),
                    _ => None,
                })?;
            vector_type(types, lanes, element)
        }
        5 | 21 | 22 => integer_type(types, 64),
        19 => first.and_then(|ty| split_vector_type(types, ty)),
        20 => {
            let i1 = integer_type(types, 1)?;
            type_index(
                types,
                |ty| matches!(ty, BinaryType::ScalableVector { element } if *element == i1),
            )
        }
        23 => value_type_at(instruction, 1, value_types)
            .and_then(|shape| vector_shape_type(types, shape, integer_type(types, 8)?)),
        _ => first,
    }
}

fn infer_vararg_result(instruction: &BinaryInstruction, types: &BinaryTypeTable) -> Option<u64> {
    match instruction.opcode {
        0x211 => explicit_type(instruction),
        0x214 => integer_type(types, 32),
        _ => None,
    }
}

fn intrinsic_result(
    instruction: &BinaryInstruction,
    first: Option<u64>,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<u64> {
    let id = index_operand(instruction)?;
    match id {
        0 | 1 => first,
        2..=6 => first,
        12 | 13 => integer_type(types, 64),
        15 => value_type_at(instruction, 1, value_types),
        16 => integer_type(types, 1),
        _ => None,
    }
}

fn direct_call_result(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    let signature = signatures.records.get(usize::try_from(index).ok()?)?;
    let arguments = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Types(types) => Some(types.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    if arguments.len() != signature.generic_parameters.len() {
        return None;
    }
    let function_type = specialize_function_type(types, signature, arguments)?;
    let BinaryType::Function(function) = type_at(types, function_type)? else {
        return None;
    };
    if signature.coroutine.is_some() {
        return None;
    }
    if function.asynchronous {
        coroutines::frame_type(types, index)
    } else {
        function_result(types, function_type)
    }
}

fn function_reference_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    let signature = signatures.records.get(usize::try_from(index).ok()?)?;
    let arguments = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Types(types) => Some(types.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    if arguments.len() != signature.generic_parameters.len() {
        return None;
    }
    specialize_function_type(types, signature, arguments)
}

pub(super) fn specialize_function_type(
    types: &BinaryTypeTable,
    signature: &tir_format::BinarySignature,
    arguments: &[u64],
) -> Option<u64> {
    if arguments.is_empty() {
        Some(signature.function_type)
    } else {
        address::substitute_type(types, signature.function_type, arguments)
    }
}

fn gpu_entry_function_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
) -> Option<u64> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    let signature = signatures.records.get(usize::try_from(index).ok()?)?;
    if !signature.generic_parameters.is_empty()
        || instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, BinaryOperand::Types(types) if !types.is_empty()))
    {
        None
    } else {
        Some(signature.function_type)
    }
}

fn function_result(types: &BinaryTypeTable, function_type: u64) -> Option<u64> {
    let Some(BinaryType::Function(function)) = type_at(types, function_type) else {
        return None;
    };
    matches!(
        type_at(types, function.result),
        Some(BinaryType::Unit | BinaryType::Never)
    )
    .then_some(None)
    .unwrap_or(Some(function.result))
}

fn callable_result(types: &BinaryTypeTable, function_type: u64) -> Option<u64> {
    let BinaryType::Function(function) = type_at(types, function_type)? else {
        return None;
    };
    if function.asynchronous {
        None
    } else {
        function_result(types, function_type)
    }
}

fn closure_result(types: &BinaryTypeTable, closure: u64) -> Option<u64> {
    let BinaryType::Closure { function_type, .. } = type_at(types, closure)? else {
        return None;
    };
    callable_result(types, *function_type)
}

fn infer_vector_type(types: &BinaryTypeTable, vector: u64) -> Option<u64> {
    type_at(types, vector).and_then(|ty| match ty {
        BinaryType::FixedVector { element, .. } | BinaryType::ScalableVector { element } => {
            Some(*element)
        }
        _ => None,
    })
}

fn vector_element_type(types: &BinaryTypeTable, vector: u64) -> Option<u64> {
    infer_vector_type(types, vector)
}

fn tensor_element_type(types: &BinaryTypeTable, tensor: u64) -> Option<u64> {
    match type_at(types, tensor)? {
        BinaryType::Tensor { element, .. } => Some(*element),
        _ => None,
    }
}

fn matrix_element_type(types: &BinaryTypeTable, matrix: u64) -> Option<u64> {
    match type_at(types, matrix)? {
        BinaryType::CooperativeMatrix { element, .. } => Some(*element),
        _ => None,
    }
}

fn split_vector_type(types: &BinaryTypeTable, vector: u64) -> Option<u64> {
    let BinaryType::FixedVector { lanes, element } = type_at(types, vector)? else {
        return None;
    };
    if lanes % 2 != 0 {
        return None;
    }
    vector_type(types, lanes / 2, *element)
}

fn vector_shape_type(types: &BinaryTypeTable, shape: u64, element: u64) -> Option<u64> {
    match type_at(types, shape)? {
        BinaryType::FixedVector { lanes, .. } => vector_type(types, *lanes, element),
        BinaryType::ScalableVector { .. } => type_index(
            types,
            |ty| matches!(ty, BinaryType::ScalableVector { element: actual } if *actual == element),
        ),
        _ => None,
    }
}

fn infer_concat_result(
    first: Option<u64>,
    second: Option<u64>,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let (
        BinaryType::FixedVector {
            lanes: left,
            element,
        },
        BinaryType::FixedVector {
            lanes: right,
            element: right_element,
        },
    ) = (type_at(types, first?)?, type_at(types, second?)?)
    else {
        return None;
    };
    if element != right_element {
        return None;
    }
    vector_type(types, left.checked_add(*right)?, *element)
}

fn stack_allocation_type(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    pointee: u64,
    target: Option<BinaryTarget>,
) -> Option<u64> {
    let selector = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .unwrap_or(0);
    let space = match selector {
        0 => match target? {
            BinaryTarget::Cpu | BinaryTarget::Both => 0,
            BinaryTarget::Gpu => 4,
        },
        1..=9 => u8::try_from(selector).ok()?,
        10 => 0,
        _ => return None,
    };
    address_type(types, pointee, space)
}
