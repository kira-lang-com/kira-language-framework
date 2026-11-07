use std::collections::HashMap;
use tir_types::Type;

use crate::text_function_constant_assembly::parse_entry_constant;
use crate::text_integer_operation_assembly::{integer_operation_info, parse_integer_operation};
use crate::text_select_assembly::parse_select;
use crate::text_straight_line_assembly::error;
use crate::{TextItem, TextParseError};

pub(crate) fn validate_operations(
    instructions: &[crate::TextInstruction],
    values: &mut HashMap<String, Type>,
    source: &str,
    item: TextItem,
    callees: Option<&crate::text_direct_call::Callees>,
) -> Result<(), TextParseError> {
    for instruction in instructions {
        if crate::text_straight_line_constants::validate(
            instruction,
            values,
            source,
            item,
            callees,
        )? {
            continue;
        }
        if crate::text_pointer_address::handles(&instruction.opcode) {
            let operation = crate::text_pointer_address::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_pointer_address::validate(&operation, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate pointer/address SSA result",
                ));
            }
            continue;
        }
        if crate::text_enum_address::handles(&instruction.opcode) {
            let operation = crate::text_enum_address::parse(instruction, source, item)?;
            if let Some((name, ty)) = crate::text_enum_address::validate(
                &operation,
                values,
                callees,
                item,
                instruction.span,
            )? {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate enum address result",
                    ));
                }
            }
            continue;
        }
        if crate::text_address_projection::handles(&instruction.opcode) {
            let operation = crate::text_address_projection::parse(instruction, source, item)?;
            let (name, ty) = crate::text_address_projection::validate(
                &operation,
                values,
                callees,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate projected address SSA result",
                ));
            }
            continue;
        }
        if crate::text_enum_payload::handles(&instruction.opcode) {
            let operation = crate::text_enum_payload::parse(instruction, source, item)?;
            let (name, ty) = crate::text_enum_payload::validate(
                &operation,
                values,
                callees,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate enum payload SSA result",
                ));
            }
            continue;
        }
        if crate::text_aggregate_destructure::handles(&instruction.opcode) {
            let operation = crate::text_aggregate_destructure::parse(instruction, source, item)?;
            let results = crate::text_aggregate_destructure::validate(
                &operation,
                values,
                callees,
                item,
                instruction.span,
            )?;
            for (name, ty) in results {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate destructure SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_nominal_construction::handles(&instruction.opcode) {
            let operation = crate::text_nominal_construction::parse(instruction, source, item)?;
            let (name, ty) = crate::text_nominal_construction::validate(
                &operation,
                values,
                callees,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate nominal SSA result",
                ));
            }
            continue;
        }
        if crate::text_vector_gather::handles(&instruction.opcode) {
            let operation = crate::text_vector_gather::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_vector_gather::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate vec.gather SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_vector_memory::handles(&instruction.opcode) {
            let operation = crate::text_vector_memory::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_vector_memory::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate vec.load SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_managed_heap::handles(&instruction.opcode) {
            let managed = crate::text_managed_heap::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_managed_heap::result_type(&managed, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate managed heap SSA result",
                ));
            }
            continue;
        }
        if crate::text_global_reference::handles(&instruction.opcode) {
            let global = crate::text_global_reference::parse(instruction, source, item)?;
            let symbols = callees.ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "global reference requires module-wide GLOBALS",
                )
            })?;
            let (name, ty) = crate::text_global_reference::validate(
                &global,
                values,
                symbols,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate global reference SSA result",
                ));
            }
            continue;
        }
        if instruction.opcode == "descriptor.ref" {
            let descriptor = crate::text_descriptor_reference::parse(instruction, source, item)?;
            let (name, ty) = crate::text_descriptor_reference::validate(
                &descriptor,
                values,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate descriptor.ref SSA result",
                ));
            }
            continue;
        }
        if instruction.opcode == "func.ref" {
            let reference = crate::text_function_reference::parse(instruction, source, item)?;
            let callees = callees.ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "func.ref requires module-wide SIGNATURES",
                )
            })?;
            let (name, ty) = crate::text_function_reference::validate(
                &reference,
                values,
                callees,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate func.ref SSA result",
                ));
            }
            continue;
        }
        if instruction.opcode == "block.addr" {
            let address = crate::text_block_address::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_block_address::validate(&address, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate block.addr SSA result",
                ));
            }
            continue;
        }
        if instruction.opcode == "call" {
            let call = crate::text_direct_call::parse(instruction, source, item)?;
            let callees = callees.ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "direct call requires callee signatures",
                )
            })?;
            if let Some((name, ty)) =
                crate::text_direct_call::validate(&call, values, callees, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate direct call SSA result",
                    ));
                }
            }
            continue;
        }
        if instruction.opcode == "call.indirect" {
            let call = crate::text_indirect_call::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_indirect_call::validate(&call, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate indirect-call SSA result",
                    ));
                }
            }
            continue;
        }

        if crate::text_simple_instructions::handles(&instruction.opcode) {
            let operation = crate::text_simple_instructions::parse(instruction, source, item)?;
            if let Some((name, ty)) = crate::text_simple_instructions::validate(
                &operation,
                values,
                item,
                instruction.span,
            )? {
                if values.insert(name, ty).is_some() {
                    return Err(error(item, instruction.span, "duplicate SSA result"));
                }
            }
            continue;
        }
        if crate::text_tuple_simple::handles(&instruction.opcode) {
            let operation = crate::text_tuple_simple::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_tuple_simple::validate(&operation, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(item, instruction.span, "duplicate tuple SSA result"));
            }
            continue;
        }
        if crate::text_array_simple::handles(&instruction.opcode) {
            let operation = crate::text_array_simple::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_array_simple::validate(&operation, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(item, instruction.span, "duplicate array SSA result"));
            }
            continue;
        }
        if crate::text_raw_memory::opcode(&instruction.opcode).is_some() {
            let operation = crate::text_raw_memory::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_raw_memory::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate raw pointer SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_raw_access::handles(&instruction.opcode) {
            let operation = crate::text_raw_access::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_raw_access::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate raw access SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_device_io::handles(&instruction.opcode) {
            let operation = crate::text_device_io::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_device_io::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(item, instruction.span, "duplicate MMIO SSA result"));
                }
            }
            continue;
        }
        if crate::text_raw_heap::handles(&instruction.opcode) {
            let operation = crate::text_raw_heap::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_raw_heap::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate raw heap SSA result",
                    ));
                }
            }
            continue;
        }
        if crate::text_fp_misc::handles(&instruction.opcode) {
            let operation = crate::text_fp_misc::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_fp_misc::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(item, instruction.span, "duplicate FP SSA result"));
                }
            }
            continue;
        }
        if crate::text_atomic_simple::handles(&instruction.opcode) {
            let operation = crate::text_atomic_simple::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_atomic_simple::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(item, instruction.span, "duplicate atomic SSA result"));
                }
            }
            continue;
        }
        if crate::text_atomic_wait_notify::handles(&instruction.opcode) {
            let operation = crate::text_atomic_wait_notify::parse(instruction, source, item)?;
            let (name, ty) = crate::text_atomic_wait_notify::validate(
                &operation,
                values,
                item,
                instruction.span,
            )?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate atomic wait/notify SSA result",
                ));
            }
            continue;
        }
        if crate::text_atomic_update::handles(&instruction.opcode) {
            let operation = crate::text_atomic_update::parse(instruction, source, item)?;
            let (name, ty) =
                crate::text_atomic_update::validate(&operation, values, item, instruction.span)?;
            if values.insert(name, ty).is_some() {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate atomic update SSA result",
                ));
            }
            continue;
        }
        if crate::text_varargs_simple::handles(&instruction.opcode) {
            let operation = crate::text_varargs_simple::parse(instruction, source, item)?;
            if let Some((name, ty)) =
                crate::text_varargs_simple::validate(&operation, values, item, instruction.span)?
            {
                if values.insert(name, ty).is_some() {
                    return Err(error(
                        item,
                        instruction.span,
                        "duplicate variadic SSA result",
                    ));
                }
            }
            continue;
        }
        let (name, ty) = if integer_operation_info(&instruction.opcode).is_some() {
            let parsed = parse_integer_operation(instruction, item)?;
            let types = parsed
                .operands
                .iter()
                .map(|name| {
                    values.get(name).ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "integer operation references an undefined SSA value",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if parsed.opcode == "icmp" {
                let comparable = matches!(types[0], Type::Integer(_) | Type::Pointer(_))
                    || matches!(types[0], Type::FixedVector { element, .. }
                        if matches!(element.as_ref(), Type::Integer(_)));
                if !comparable
                    || types.iter().any(|ty| *ty != types[0])
                    || (matches!(types[0], Type::Pointer(_))
                        && matches!(parsed.modifiers >> 48, 2..=5))
                {
                    return Err(error(
                        item,
                        instruction.span,
                        "icmp operands must have the same integer or pointer type",
                    ));
                }
                let result_ty = match types[0] {
                    Type::FixedVector { lanes, .. } => Type::FixedVector {
                        lanes: *lanes,
                        element: Box::new(Type::Integer(1)),
                    },
                    _ => Type::Integer(1),
                };
                (parsed.name, result_ty)
            } else if parsed.opcode == "freeze" {
                let supported = match types[0] {
                    Type::Integer(_) | Type::Float(_) | Type::Pointer(_) => true,
                    Type::FixedVector { element, .. } => {
                        matches!(element.as_ref(), Type::Integer(_) | Type::Float(_))
                    }
                    _ => false,
                };
                if !supported || types.len() != 1 {
                    return Err(error(
                        item,
                        instruction.span,
                        "freeze requires a loadable scalar or fixed numeric vector",
                    ));
                }
                (parsed.name, (*types[0]).clone())
            } else {
                let integer_shape = matches!(types[0], Type::Integer(_))
                    || matches!(types[0], Type::FixedVector { element, .. }
                        if matches!(element.as_ref(), Type::Integer(_)));
                if !integer_shape || types.iter().any(|ty| *ty != types[0]) {
                    return Err(error(
                        item,
                        instruction.span,
                        "integer operation operands must have the same integer scalar/vector type",
                    ));
                }
                let width = match types[0] {
                    Type::Integer(bits) => Some(u64::from(*bits)),
                    Type::FixedVector { element, .. } => match element.as_ref() {
                        Type::Integer(bits) => Some(u64::from(*bits)),
                        _ => None,
                    },
                    _ => None,
                };
                if (parsed.opcode == "bswap" && !width.is_some_and(|bits| bits % 16 == 0))
                    || (matches!(parsed.opcode.as_str(), "clmul" | "clmulh")
                        && (!matches!(width, Some(32 | 64))
                            || !matches!(types[0], Type::Integer(_))))
                {
                    return Err(error(
                        item,
                        instruction.span,
                        "bswap requires a width divisible by 16; clmul/clmulh require i32 or i64",
                    ));
                }
                (parsed.name, (*types[0]).clone())
            }
        } else if crate::text_float_assembly::operation_info(&instruction.opcode).is_some() {
            let parsed = crate::text_float_assembly::parse(instruction, item)?;
            let operands = parsed
                .operands
                .iter()
                .map(|name| {
                    values.get(name).ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "floating-point operand is undefined",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let float_shape = match operands[0] {
                Type::Float(_) => true,
                Type::FixedVector { element, .. } => matches!(element.as_ref(), Type::Float(_)),
                _ => false,
            };
            if !float_shape || operands.iter().any(|ty| *ty != operands[0]) {
                return Err(error(
                    item,
                    instruction.span,
                    "floating-point operands must have the same float scalar or vector type",
                ));
            }
            let ty = if parsed.is_comparison {
                match operands[0] {
                    Type::FixedVector { lanes, .. } => Type::FixedVector {
                        lanes: *lanes,
                        element: Box::new(Type::Integer(1)),
                    },
                    _ => Type::Integer(1),
                }
            } else {
                (*operands[0]).clone()
            };
            (parsed.result, ty)
        } else if crate::text_vector_permute::handles(&instruction.opcode) {
            let operation = crate::text_vector_permute::parse(instruction, source, item)?;
            crate::text_vector_permute::infer(&operation, values, item, instruction.span)?
        } else if crate::text_vector_simple::opcode(&instruction.opcode).is_some() {
            let parsed = crate::text_vector_simple::parse(instruction, source, item)?;
            let ty = crate::text_vector_simple::infer(&parsed, values, item, instruction.span)?;
            (parsed.result, ty)
        } else if crate::text_conversion_assembly::opcode(&instruction.opcode).is_some() {
            let conversion = crate::text_conversion_assembly::parse(instruction, source, item)?;
            let operand = values
                .get(&conversion.source)
                .ok_or_else(|| error(item, instruction.span, "undefined conversion operand"))?;
            if !crate::text_conversion_assembly::validate(
                operand,
                &conversion.target,
                conversion.opcode,
            ) {
                return Err(error(
                    item,
                    instruction.span,
                    "conversion source and target types are incompatible",
                ));
            }
            (conversion.result, conversion.target)
        } else if matches!(
            instruction.opcode.as_str(),
            "const.int"
                | "const.bool"
                | "const.zero"
                | "const.float"
                | "const.null"
                | "const.poison"
                | "const.bytes"
                | "const.string"
        ) {
            let constant = parse_entry_constant(instruction, source, item)?;
            (constant.name().to_owned(), constant.ty().clone())
        } else if crate::text_memory_assembly::handles(&instruction.opcode) {
            let parsed = crate::text_memory_assembly::parse(instruction, source, item)?;
            let result_ty = parsed.result_type(values, item, instruction.span)?;
            let Some(ty) = result_ty else {
                continue;
            };
            let name = instruction
                .results
                .first()
                .ok_or_else(|| error(item, instruction.span, "memory result is missing"))?;
            (name.clone(), ty)
        } else if crate::text_access_assembly::handles(&instruction.opcode) {
            let parsed = crate::text_access_assembly::parse(instruction, item)?;
            if parsed.opcode == 0x187 {
                if !matches!(values.get(&parsed.operand), Some(Type::Address { .. })) {
                    return Err(error(
                        item,
                        instruction.span,
                        "access.begin requires a typed address",
                    ));
                }
            } else if values.get(&parsed.operand) != Some(&Type::Unit) {
                return Err(error(
                    item,
                    instruction.span,
                    "access.end requires an access token",
                ));
            }
            let Some(name) = parsed.result else {
                continue;
            };
            (name, Type::Unit)
        } else if matches!(
            instruction.opcode.as_str(),
            "copy" | "move" | "destroy" | "borrow.begin" | "borrow.end"
        ) {
            let parsed = crate::text_ownership_assembly::parse(instruction, item)?;
            let operand_type = values
                .get(&parsed.operand)
                .ok_or_else(|| error(item, instruction.span, "ownership operand is undefined"))?
                .clone();
            let Some(name) = parsed.result else {
                continue;
            };
            (name, operand_type)
        } else if instruction.opcode == "select" {
            let parsed = parse_select(instruction, item)?;
            let condition = values
                .get(&parsed.condition)
                .ok_or_else(|| error(item, instruction.span, "select condition is undefined"))?;
            let when_true = values
                .get(&parsed.when_true)
                .ok_or_else(|| error(item, instruction.span, "select true operand is undefined"))?;
            let when_false = values.get(&parsed.when_false).ok_or_else(|| {
                error(item, instruction.span, "select false operand is undefined")
            })?;
            let expected_mask = match when_true {
                Type::FixedVector { lanes, .. } => Type::FixedVector {
                    lanes: *lanes,
                    element: Box::new(Type::Integer(1)),
                },
                _ => Type::Integer(1),
            };
            if condition != &expected_mask || when_true != when_false {
                return Err(error(
                    item,
                    instruction.span,
                    "select requires a matching i1 scalar/vector mask and equal value types",
                ));
            }
            (parsed.name, when_true.clone())
        } else {
            return Err(error(
                item,
                instruction.span,
                "straight-line assembly supports integer operations, integer/boolean constants and select",
            ));
        };
        if values.insert(name, ty).is_some() {
            return Err(error(
                item,
                instruction.span,
                "SSA value is defined more than once",
            ));
        }
    }
    Ok(())
}
