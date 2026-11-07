use std::fmt::Write as _;

pub(crate) use crate::text_constant_type_name::constant_type_name;
use crate::text_straight_line_operand_helpers::{
    append_operation, formatting_error, get_type, get_value, malformed, mask_type_index,
    select_operands, single_block, value_operands,
};
use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryFunctionBody, BinaryFunctionType,
    BinaryGlobalTable, BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType,
    BinaryTypeTable,
};
use crate::{FormatError, StringTable};

pub(crate) fn format(
    text: &mut String,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    constants: &BinaryConstantTable,
    globals: &BinaryGlobalTable,
    strings: &StringTable,
) -> Result<(), FormatError> {
    let (block, instructions) = single_block(body, function)?;
    text.push_str(" {\n  bb0(");
    let mut values = Vec::new();
    for (index, parameter) in block.parameters.iter().enumerate() {
        let signature = &function.parameters[index];
        if parameter.ty != signature.ty
            || parameter.convention != signature.convention
            || parameter.context != signature.context
        {
            return Err(malformed(
                "entry block parameters do not match function signature",
            ));
        }
        if index != 0 {
            text.push_str(", ");
        }
        write!(text, "%arg{index}: ").map_err(formatting_error)?;
        if parameter.context {
            text.push_str("context ");
        }
        match parameter.convention {
            0 => text.push_str("owned "),
            1 => text.push_str("borrow "),
            2 => text.push_str("borrowmut "),
            3 => {}
            _ => return Err(malformed("unsupported block parameter convention")),
        }
        text.push_str(&crate::text_type_disassembly::format_type_reference(
            types,
            parameter.ty,
            signatures,
        )?);
        values.push((format!("arg{index}"), parameter.ty));
    }
    text.push_str("):\n");
    let Some((terminator, operations)) = instructions.split_last() else {
        return Err(malformed("CODE block contains no terminator"));
    };
    format_operations(
        text,
        operations,
        types,
        signatures,
        constants,
        globals,
        strings,
        &mut values,
    )?;
    if terminator.opcode != 0x1e5 || terminator.modifiers != 0 || terminator.result_count != 0 {
        return Err(malformed("straight-line CODE must end with return"));
    }
    text.push_str("    return");
    match (
        get_type(types, function.result)?,
        terminator.operands.as_slice(),
    ) {
        (BinaryType::Unit, []) => {}
        (BinaryType::Unit, _) => return Err(malformed("unit return must not have an operand")),
        (_, [BinaryOperand::Value(value)]) => {
            let (name, ty) = get_value(&values, *value)?;
            if ty != function.result {
                return Err(malformed(
                    "return value type does not match function result",
                ));
            }
            write!(text, " %{name}").map_err(formatting_error)?;
        }
        _ => return Err(malformed("return expects one SSA value")),
    }
    text.push_str("\n  }");
    Ok(())
}

pub(crate) fn format_operations(
    text: &mut String,
    operations: &[BinaryInstruction],
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    constants: &BinaryConstantTable,
    globals: &BinaryGlobalTable,
    strings: &StringTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let i1_type = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Integer(1)))
        .map(|value| value as u64);
    for operation in operations {
        if matches!(operation.opcode, 0x402 | 0x403) {
            crate::text_gpu_sync::format(text, operation)?;
            continue;
        }
        if crate::text_straight_line_constants::format(
            text, operation, types, constants, strings, values,
        )? {
            continue;
        }
        if matches!(operation.opcode, 0x126..=0x128) {
            crate::text_pointer_address::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x124 | 0x125) {
            crate::text_enum_address::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x122 | 0x123) {
            crate::text_address_projection::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x10c | 0x10d) {
            crate::text_enum_payload::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x103 | 0x106) {
            crate::text_aggregate_destructure::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x100 | 0x101 | 0x102 | 0x10a | 0x10b) {
            crate::text_nominal_disassembly::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x0c9 | 0x0ca) {
            crate::text_vector_gather::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x0c7 | 0x0c8) {
            crate::text_vector_memory::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x0c3 | 0x0c6) {
            crate::text_vector_permute::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x160..=0x163 | 0x167..=0x16a) {
            crate::text_managed_heap::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x00b | 0x00c) {
            crate::text_global_reference::format(text, operation, globals, types, values)?;
            continue;
        }
        if operation.opcode == 0x00d {
            crate::text_descriptor_reference::format(text, operation, types, signatures, values)?;
            continue;
        }
        if operation.opcode == 0x00a {
            crate::text_function_reference::format(text, operation, types, signatures, values)?;
            continue;
        }
        if operation.opcode == 0x00f {
            crate::text_block_address::format(text, operation, types, values)?;
            continue;
        }
        if operation.opcode == 0x1a0 {
            crate::text_direct_call::format(text, operation, types, signatures, values)?;
            continue;
        }
        if operation.opcode == 0x1a1 {
            crate::text_indirect_call::format(text, operation, types, signatures, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x210..=0x214) {
            crate::text_varargs_simple::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x202 | 0x203) {
            crate::text_atomic_update::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x205 | 0x206) {
            crate::text_atomic_wait_notify::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x200 | 0x201 | 0x204) {
            crate::text_atomic_simple::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x075..=0x079) {
            crate::text_fp_misc::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x164..=0x166) {
            crate::text_raw_heap::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x270..=0x272) {
            crate::text_device_io::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x146 | 0x147) {
            crate::text_raw_access::format(text, operation, types, values)?;
            continue;
        }
        if crate::text_raw_memory::name(operation.opcode).is_some() {
            crate::text_raw_memory::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x107..=0x109) {
            crate::text_array_simple::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x104 | 0x105) {
            crate::text_tuple_simple::format(text, operation, types, values)?;
            continue;
        }
        if matches!(
            operation.opcode,
            0x223 | 0x224 | 0x230 | 0x243 | 0x244 | 0x245 | 0x246
        ) {
            crate::text_simple_instructions::format(text, operation, types, strings, values)?;
            continue;
        }
        if crate::text_vector_simple::name(operation.opcode).is_some() {
            crate::text_vector_simple::format(text, operation, types, values)?;
            continue;
        }
        if crate::text_conversion_assembly::name(operation.opcode).is_some() {
            crate::text_conversion_disassembly::format(text, operation, types, values)?;
            continue;
        }
        if operation.opcode == 0x006 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed("invalid const.string instruction flags"));
            }
            let [BinaryOperand::Index(index)] = operation.operands.as_slice() else {
                return Err(malformed("const.string requires one STRINGS index"));
            };
            let value = usize::try_from(*index)
                .ok()
                .and_then(|index| strings.get(index))
                .ok_or_else(|| malformed("const.string index is out of range"))?;
            let ty = types
                .entries
                .iter()
                .enumerate()
                .find_map(|(index, entry)| {
                    if let BinaryType::Tuple(elements) = entry
                        && elements.len() == 2
                        && matches!(get_type(types, elements[0]), Ok(BinaryType::Pointer(0)))
                        && matches!(get_type(types, elements[1]), Ok(BinaryType::Integer(64)))
                    {
                        Some(index as u64)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| malformed("const.string `(ptr, i64)` type is not interned"))?;
            let literal = crate::text_global_disassembly::quote(value)?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = const.string {literal}").map_err(formatting_error)?;
            values.push((name, ty));
            continue;
        }
        if operation.opcode == 0x007 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed("invalid const.bytes instruction flags"));
            }
            let [BinaryOperand::Type(ty), BinaryOperand::Bytes(payload)] =
                operation.operands.as_slice()
            else {
                return Err(malformed("const.bytes requires a type and byte payload"));
            };
            let (name, expected_bytes) = match get_type(types, *ty)? {
                BinaryType::Integer(bits) if bits % 8 == 0 => {
                    (format!("i{bits}"), usize::try_from(bits / 8).ok())
                }
                BinaryType::Float(float) if float.bits() % 8 == 0 => {
                    (float.to_string(), Some(usize::from(float.bits() / 8)))
                }
                _ => return Err(malformed("const.bytes requires a byte-aligned scalar type")),
            };
            if expected_bytes != Some(payload.len()) {
                return Err(malformed(
                    "const.bytes payload length does not match its type",
                ));
            }
            let result = format!("v{}", values.len());
            write!(text, "    %{result} = const.bytes {name}, b\"").map_err(formatting_error)?;
            for byte in payload {
                write!(text, "\\x{byte:02X}").map_err(formatting_error)?;
            }
            text.push_str("\"\n");
            values.push((result, *ty));
            continue;
        }
        if operation.opcode == 0x003 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed(
                    "const.null has invalid modifiers or result count",
                ));
            }
            let [BinaryOperand::Index(space)] = operation.operands.as_slice() else {
                return Err(malformed("const.null requires an address-space index"));
            };
            let suffix = match *space {
                0 => "",
                1 => " device",
                2 => " constant",
                3 => " threadgroup",
                4 => " thread",
                5 => " object_payload",
                6 => " ray_payload",
                7 => " hit_attribute",
                8 => " tile",
                9 => " node_record",
                _ => return Err(malformed("const.null address-space index is invalid")),
            };
            let ty = types
                .entries
                .iter()
                .position(|entry| {
                    matches!(entry,
                BinaryType::Pointer(encoded) if u64::from(*encoded) == *space)
                })
                .ok_or_else(|| malformed("const.null pointer type is missing"))?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = const.null{suffix}").map_err(formatting_error)?;
            values.push((name, ty as u64));
            continue;
        }
        if operation.opcode == 0x005 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed(
                    "const.poison has invalid modifiers or result count",
                ));
            }
            let [BinaryOperand::Type(ty)] = operation.operands.as_slice() else {
                return Err(malformed("const.poison requires a type index"));
            };
            let ty_text = constant_type_name(types, *ty)?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = const.poison {ty_text}").map_err(formatting_error)?;
            values.push((name, *ty));
            continue;
        }
        if operation.opcode == 0x001 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed("invalid const.float instruction flags"));
            }
            let [BinaryOperand::Type(ty), BinaryOperand::Index(index)] =
                operation.operands.as_slice()
            else {
                return Err(malformed("const.float requires a type and constant index"));
            };
            let Some(BinaryType::Float(float)) = types.entries.get(*ty as usize) else {
                return Err(malformed(
                    "const.float result must have floating-point type",
                ));
            };
            let constant = constants
                .records
                .get(*index as usize)
                .ok_or_else(|| malformed("const.float CONSTANTS index is invalid"))?;
            if constant.ty != *ty {
                return Err(malformed("const.float type disagrees with CONSTANTS"));
            }
            let BinaryConstantValue::Float(bytes) = &constant.value else {
                return Err(malformed(
                    "const.float CONSTANTS value is not floating-point",
                ));
            };
            let literal = crate::text_global_disassembly::format_ieee_float_literal(bytes, *float)
                .ok_or_else(|| malformed("const.float bytes have no canonical float text"))?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = const.float {float}, {literal}")
                .map_err(formatting_error)?;
            values.push((name, *ty));
            continue;
        }
        if crate::text_memory_disassembly::handles(operation.opcode) {
            crate::text_memory_disassembly::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x187 | 0x188) {
            let [BinaryOperand::Value(value)] = operation.operands.as_slice() else {
                return Err(malformed("access instruction expects one SSA value"));
            };
            let (operand, ty) = get_value(values, *value)?;
            if operation.opcode == 0x187 {
                if operation.result_count != 1 || operation.modifiers & !(0xf << 48) != 0 {
                    return Err(malformed("access.begin has invalid modifiers or results"));
                }
                if !matches!(get_type(types, ty)?, BinaryType::Address { .. }) {
                    return Err(malformed("access.begin requires a typed address"));
                }
                let selector = operation.modifiers >> 48;
                if selector > 5 {
                    return Err(malformed("access.begin selector is invalid"));
                }
                let mode = if selector % 2 == 0 { "read" } else { "modify" };
                let checking = match selector / 2 {
                    0 => "static",
                    1 => "dynamic",
                    2 => "unchecked",
                    _ => return Err(malformed("access.begin selector is invalid")),
                };
                let result = format!("v{}", values.len());
                writeln!(
                    text,
                    "    %{result} = access.begin [{mode}] [{checking}] %{operand}"
                )
                .map_err(formatting_error)?;
                values.push((result, u64::MAX)); // Scope tokens are not loadable types.
            } else {
                if operation.modifiers != 0 || operation.result_count != 0 || ty != u64::MAX {
                    return Err(malformed(
                        "access.end requires a scope token without results",
                    ));
                }
                writeln!(text, "    access.end %{operand}").map_err(formatting_error)?;
            }
            continue;
        }
        if operation.opcode == 0x004 {
            if operation.modifiers != 0 || operation.result_count != 1 {
                return Err(malformed(
                    "const.zero has invalid modifiers or result count",
                ));
            }
            let [BinaryOperand::Type(index)] = operation.operands.as_slice() else {
                return Err(malformed("const.zero requires one type index"));
            };
            let ty = constant_type_name(types, *index)?;
            let result = format!("v{}", values.len());
            writeln!(text, "    %{result} = const.zero {ty}").map_err(formatting_error)?;
            values.push((result, *index));
            continue;
        }
        if matches!(operation.opcode, 0x000 | 0x002) {
            if operation.result_count != 1 || operation.modifiers != 0 {
                return Err(malformed("constant has invalid results or modifiers"));
            }
            let result_name = format!("v{}", values.len());
            let result_ty = if operation.opcode == 0x000 {
                let [BinaryOperand::Type(index), BinaryOperand::Bytes(bytes)] =
                    operation.operands.as_slice()
                else {
                    return Err(malformed("const.int operands are malformed"));
                };
                let BinaryType::Integer(width) = get_type(types, *index)? else {
                    return Err(malformed("const.int must have an integer type"));
                };
                let width = u16::try_from(*width)
                    .map_err(|_| malformed("const.int width is unsupported"))?;
                if width == 0
                    || width > 128
                    || bytes.len() != usize::from(width).div_ceil(8)
                    || (width % 8 != 0 && bytes.last().is_some_and(|byte| byte >> (width % 8) != 0))
                {
                    return Err(malformed("invalid const.int payload width or high bits"));
                }
                let mut raw = [0u8; 16];
                raw[..bytes.len()].copy_from_slice(bytes);
                let value = u128::from_le_bytes(raw);
                write!(text, "    %{result_name} = const.int i{width}, {value}\n")
                    .map_err(formatting_error)?;
                *index
            } else {
                let [BinaryOperand::Integer(value)] = operation.operands.as_slice() else {
                    return Err(malformed("const.bool operands are malformed"));
                };
                let literal = match (value.negative, value.magnitude) {
                    (false, 0) => "false",
                    (false, 1) => "true",
                    _ => return Err(malformed("const.bool is not false or true")),
                };
                write!(text, "    %{result_name} = const.bool {literal}\n")
                    .map_err(formatting_error)?;
                i1_type.ok_or_else(|| malformed("const.bool result i1 is not interned"))?
            };
            values.push((result_name, result_ty));
            continue;
        }
        if matches!(operation.opcode, 0x180..=0x184) {
            let [BinaryOperand::Value(value)] = operation.operands.as_slice() else {
                return Err(malformed("ownership operation expects one SSA value"));
            };
            if operation.modifiers != 0
                || operation.result_count != u64::from(!matches!(operation.opcode, 0x182 | 0x184))
            {
                return Err(malformed(
                    "invalid ownership instruction modifiers or result count",
                ));
            }
            let (operand, ty) = get_value(values, *value)?;
            if matches!(operation.opcode, 0x182 | 0x184) {
                let opcode = if operation.opcode == 0x182 {
                    "destroy"
                } else {
                    "borrow.end"
                };
                writeln!(text, "    {opcode} %{operand}").map_err(formatting_error)?;
            } else {
                let opcode = match operation.opcode {
                    0x180 => "copy",
                    0x181 => "move",
                    0x183 => "borrow.begin",
                    _ => return Err(malformed("invalid ownership instruction")),
                };
                let result_name = format!("v{}", values.len());
                writeln!(text, "    %{result_name} = {opcode} %{operand}")
                    .map_err(formatting_error)?;
                values.push((result_name, ty));
            }
            continue;
        }
        let (opcode, operands, result_ty) = if let Some((opcode, operand_count)) =
            super::text_function_body_disassembly::integer_operation_name(operation.opcode)
        {
            if !super::text_function_body_disassembly::valid_integer_operation_modifiers(operation)
            {
                return Err(malformed("unsupported integer operation modifier"));
            }
            let operands = value_operands(operation, operand_count)?;
            let first = get_value(&values, operands[0])?.1;
            if operands
                .iter()
                .any(|operand| get_value(&values, *operand).is_err())
                || operands
                    .iter()
                    .any(|operand| get_value(&values, *operand).is_ok_and(|(_, ty)| ty != first))
            {
                return Err(malformed("integer operation operands have different types"));
            }
            let operand_ty = get_type(types, first)?;
            let result_ty = if operation.opcode == 0x03c {
                let comparable =
                    matches!(operand_ty, BinaryType::Integer(_) | BinaryType::Pointer(_))
                        || matches!(operand_ty, BinaryType::FixedVector { element, .. }
                        if matches!(get_type(types, *element), Ok(BinaryType::Integer(_))));
                if !comparable
                    || (matches!(operand_ty, BinaryType::Pointer(_))
                        && matches!(operation.modifiers >> 48, 2..=5))
                {
                    return Err(malformed("invalid icmp operand type"));
                }
                mask_type_index(types, first)?
            } else if operation.opcode == 0x045 {
                let supported = match operand_ty {
                    BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_) => true,
                    BinaryType::FixedVector { element, .. } => {
                        matches!(
                            get_type(types, *element)?,
                            BinaryType::Integer(_) | BinaryType::Float(_)
                        )
                    }
                    _ => false,
                };
                if !supported {
                    return Err(malformed(
                        "freeze requires a loadable scalar or fixed numeric vector",
                    ));
                }
                first
            } else {
                let integer_shape = matches!(operand_ty, BinaryType::Integer(_))
                    || matches!(operand_ty, BinaryType::FixedVector { element, .. }
                        if matches!(get_type(types, *element), Ok(BinaryType::Integer(_))));
                if !integer_shape {
                    return Err(malformed("integer operation requires integer operands"));
                }
                let width = match operand_ty {
                    BinaryType::Integer(bits) => Some(*bits),
                    BinaryType::FixedVector { element, .. } => match get_type(types, *element)? {
                        BinaryType::Integer(bits) => Some(*bits),
                        _ => None,
                    },
                    _ => None,
                };
                if (operation.opcode == 0x041 && !width.is_some_and(|bits| bits % 16 == 0))
                    || (matches!(operation.opcode, 0x043 | 0x044)
                        && (!matches!(width, Some(32 | 64))
                            || !matches!(operand_ty, BinaryType::Integer(_))))
                {
                    return Err(malformed("integer operation has unsupported element width"));
                }
                first
            };
            let opcode = if operation.opcode == 0x03c {
                let predicate = match operation.modifiers >> 48 {
                    0 => "eq",
                    1 => "ne",
                    2 => "slt",
                    3 => "sle",
                    4 => "sgt",
                    5 => "sge",
                    6 => "ult",
                    7 => "ule",
                    8 => "ugt",
                    9 => "uge",
                    _ => return Err(malformed("invalid icmp predicate")),
                };
                format!("icmp {predicate}")
            } else {
                let flag = match operation.opcode {
                    0x028 if operation.modifiers & (1 << 3) != 0 => " [exact]",
                    0x02c if operation.modifiers & (1 << 4) != 0 => " [disjoint]",
                    0x03e | 0x03f if operation.modifiers & (1 << 47) != 0 => " [poison]",
                    _ => "",
                };
                format!("{opcode}{flag}")
            };
            (opcode, operands, result_ty)
        } else if let Some((opcode, operand_count)) =
            crate::text_float_assembly::operation_name(operation.opcode)
        {
            let allowed_modifiers = if opcode.starts_with("math.") {
                1 << 17
            } else if operation.opcode == 0x074 {
                0xf << 48
            } else {
                0
            };
            if operation.result_count != 1 || operation.modifiers & !allowed_modifiers != 0 {
                return Err(malformed(
                    "invalid float operation modifiers or result count",
                ));
            }
            let operands = value_operands(operation, operand_count)?;
            let operand_ty = get_value(&values, operands[0])?.1;
            let is_float_type = match get_type(types, operand_ty)? {
                BinaryType::Float(_) => true,
                BinaryType::FixedVector { element, .. } => {
                    matches!(get_type(types, *element)?, BinaryType::Float(_))
                }
                _ => false,
            };
            if !is_float_type
                || operands.iter().any(|operand| {
                    get_value(&values, *operand).map_or(true, |(_, ty)| ty != operand_ty)
                })
            {
                return Err(malformed(
                    "float operation operands have incompatible types",
                ));
            }
            let name = if operation.opcode == 0x074 {
                let predicate =
                    crate::text_float_assembly::predicate_name(operation.modifiers >> 48)
                        .ok_or_else(|| malformed("fcmp predicate selector is invalid"))?;
                format!("fcmp {predicate}")
            } else {
                if operation.modifiers != 0
                    && !(opcode.starts_with("math.") && operation.modifiers == (1 << 17))
                {
                    return Err(malformed("unsupported floating-point modifiers"));
                }
                if opcode.starts_with("math.") && operation.modifiers == (1 << 17) {
                    format!("{opcode} [approx]")
                } else {
                    opcode.to_owned()
                }
            };
            let result_ty = if operation.opcode == 0x074 {
                mask_type_index(types, operand_ty)?
            } else {
                operand_ty
            };
            (name, operands, result_ty)
        } else if operation.opcode == 0x03d {
            let (operands, ty) = select_operands(operation, types, values)?;
            ("select".to_owned(), operands, ty)
        } else {
            return Err(malformed("unsupported straight-line CODE instruction"));
        };
        if operation.result_count != 1 {
            return Err(malformed("straight-line operation must define one value"));
        }
        append_operation(text, &opcode, &operands, result_ty, values)?;
    }
    Ok(())
}
