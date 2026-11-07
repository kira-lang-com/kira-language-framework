//! CODE-local types not present in function signatures must be interned before
//! encoding instructions and canonical disassembly (TIR 22.6).
use std::collections::HashMap;

use tir_types::Type;

use crate::text_signature_assembly::ParsedFunction;
use crate::{TextItem, TextModule, TextParseError};

pub(crate) fn collect(
    module: &TextModule,
    functions: &[ParsedFunction],
    callees: &crate::text_direct_call::Callees,
    trailing_roots: &mut Vec<(Type, TextItem)>,
) -> Result<(), TextParseError> {
    for function in functions {
        if let Some(body) = &function.body {
            for block in body.blocks.iter().skip(1) {
                for parameter in &block.parameters {
                    trailing_roots.push((parameter.ty.clone(), function.item));
                }
            }
            // Preserve first-use order after SIGNATURES roots.
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                match instruction.opcode.as_str() {
                    "gpu.builtin" => {
                        let builtin = crate::text_gpu_builtin::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        trailing_roots.push((builtin.ty, function.item));
                    }
                    "const.sampler" => {
                        trailing_roots.push((Type::Sampler, function.item));
                    }
                    "target.pagesize" => trailing_roots.push((Type::Integer(64), function.item)),
                    "target.has" => trailing_roots.push((Type::Integer(1), function.item)),
                    "ptr.diff" => trailing_roots.push((Type::Integer(64), function.item)),
                    "fclass" | "frexp" | "fp.flags.read" => {
                        trailing_roots.push((Type::Integer(32), function.item));
                    }
                    "heap.alloc.raw" | "heap.realloc.raw" => {
                        trailing_roots.push((
                            Type::Pointer(tir_types::AddressSpace::Generic),
                            function.item,
                        ));
                    }
                    "heap.alloc.counted" | "heap.alloc.unique" => {
                        let heap = crate::text_managed_heap::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = crate::text_managed_heap::required_type(&heap) {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    "raw.load" => {
                        let access = crate::text_raw_access::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_raw_access::RawAccess::Load { ty, .. } = access {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    "atomic.load" => {
                        let operation = crate::text_atomic_simple::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_atomic_simple::AtomicOp::Load { ty, .. } = operation {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    "atomic.wait" | "atomic.notify" => {
                        trailing_roots.push((Type::Integer(32), function.item));
                    }
                    "atomic.cmpxchg" => trailing_roots.push((Type::Integer(1), function.item)),
                    "va.arg" => {
                        let operation = crate::text_varargs_simple::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_varargs_simple::Varargs::Arg { ty, .. } = operation {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    "setjmp" => trailing_roots.push((Type::Integer(32), function.item)),
                    "block.addr" => trailing_roots.push((
                        Type::Pointer(tir_types::AddressSpace::Generic),
                        function.item,
                    )),
                    "descriptor.ref" => {
                        let descriptor = crate::text_descriptor_reference::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        trailing_roots.push((descriptor.ty, function.item));
                        trailing_roots.push((
                            Type::Pointer(tir_types::AddressSpace::Generic),
                            function.item,
                        ));
                    }
                    "global.addr" | "tls.addr" => trailing_roots.push((
                        Type::Pointer(tir_types::AddressSpace::Generic),
                        function.item,
                    )),
                    "mmio.load" => {
                        let access = crate::text_device_io::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_device_io::DeviceIo::Load { ty, .. } = access {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    "array.new" => {
                        let parsed = crate::text_array_simple::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_array_simple::ArrayOp::New { ty, .. } = parsed {
                            trailing_roots.push((ty, function.item));
                        }
                    }
                    opcode if crate::text_nominal_construction::handles(opcode) => {
                        let op = crate::text_nominal_construction::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = crate::text_nominal_construction::nominal_type(&op) {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                    }
                    "intrinsic" if !instruction.results.is_empty() => {
                        trailing_roots.push((Type::Integer(64), function.item));
                    }
                    opcode if crate::text_vector_simple::opcode(opcode).is_some() => {
                        let parsed = crate::text_vector_simple::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = parsed.ty {
                            trailing_roots.push((ty, function.item));
                        }
                        if matches!(parsed.opcode, 0x0c5 | 0x0d5 | 0x0d6) {
                            trailing_roots.push((Type::Integer(64), function.item));
                        }
                    }
                    "vec.load" => {
                        let op = crate::text_vector_memory::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = crate::text_vector_memory::load_type(&op) {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                    }
                    "vec.gather" => {
                        let op = crate::text_vector_gather::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = crate::text_vector_gather::load_type(&op) {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                    }
                    opcode if crate::text_conversion_assembly::opcode(opcode).is_some() => {
                        let conversion = crate::text_conversion_assembly::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        trailing_roots.push((conversion.target, function.item));
                    }
                    "const.aggregate" => {
                        let constant = crate::text_aggregate_constant::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                            Some(callees),
                        )?;
                        trailing_roots.push((constant.ty, function.item));
                    }
                    "const.vec" => {
                        let vector = crate::text_vector_constant::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        trailing_roots.push((vector.ty, function.item));
                    }
                    "addr.cast" | "ptr.toaddr" => {
                        let operation = crate::text_pointer_address::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let Some(ty) = crate::text_pointer_address::destination_type(&operation)
                        {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                    }
                    "stack.alloc" => {
                        let memory = crate::text_memory_assembly::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_memory_assembly::MemoryOperation::StackAlloc { ty } =
                            memory
                        {
                            trailing_roots.push((
                                Type::Address {
                                    pointee: Box::new(ty),
                                    space: None,
                                },
                                function.item,
                            ));
                        }
                    }
                    "icmp" | "fcmp" | "const.bool" => {
                        trailing_roots.push((Type::Integer(1), function.item))
                    }
                    "const.int" | "const.zero" | "const.null" | "const.poison" | "const.bytes"
                    | "const.string"
                        if body.blocks.iter().any(|block| block.instructions.len() > 1) =>
                    {
                        let constant =
                            crate::text_function_constant_assembly::parse_entry_constant(
                                instruction,
                                module.item_text(function.item),
                                function.item,
                            )?;
                        trailing_roots.push((constant.ty().clone(), function.item));
                    }
                    _ => {}
                }
            }
            if body.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(instruction.opcode.as_str(), "icmp" | "fcmp")
                        || crate::text_vector_simple::opcode(&instruction.opcode).is_some()
                        || crate::text_vector_permute::handles(&instruction.opcode)
                        || crate::text_tuple_simple::handles(&instruction.opcode)
                        || crate::text_array_simple::handles(&instruction.opcode)
                        || crate::text_nominal_construction::handles(&instruction.opcode)
                        || crate::text_aggregate_destructure::handles(&instruction.opcode)
                        || crate::text_enum_payload::handles(&instruction.opcode)
                        || crate::text_address_projection::handles(&instruction.opcode)
                        || crate::text_enum_address::handles(&instruction.opcode)
                        || crate::text_pointer_address::handles(&instruction.opcode)
                        || crate::text_fp_misc::handles(&instruction.opcode)
                        || crate::text_atomic_update::handles(&instruction.opcode)
                        || crate::text_managed_heap::handles(&instruction.opcode)
                })
            }) {
                let mut values: HashMap<String, Type> = body
                    .blocks
                    .iter()
                    .flat_map(|block| {
                        block
                            .parameters
                            .iter()
                            .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
                    })
                    .collect();
                for block in &body.blocks {
                    let Some((_, operations)) = block.instructions.split_last() else {
                        continue;
                    };
                    crate::text_straight_line_assembly::validate_operations(
                        operations,
                        &mut values,
                        module.item_text(function.item),
                        function.item,
                        Some(callees),
                    )?;
                    for instruction in operations.iter().filter(|op| {
                        matches!(op.opcode.as_str(), "icmp" | "fcmp")
                            || crate::text_vector_simple::opcode(&op.opcode).is_some()
                            || crate::text_vector_permute::handles(&op.opcode)
                            || crate::text_tuple_simple::handles(&op.opcode)
                            || crate::text_array_simple::handles(&op.opcode)
                            || crate::text_nominal_construction::handles(&op.opcode)
                            || crate::text_aggregate_destructure::handles(&op.opcode)
                            || crate::text_enum_payload::handles(&op.opcode)
                            || crate::text_address_projection::handles(&op.opcode)
                            || crate::text_enum_address::handles(&op.opcode)
                            || crate::text_pointer_address::handles(&op.opcode)
                            || crate::text_fp_misc::handles(&op.opcode)
                            || crate::text_atomic_update::handles(&op.opcode)
                            || crate::text_managed_heap::handles(&op.opcode)
                    }) {
                        if let Some(mask) = instruction
                            .results
                            .first()
                            .and_then(|name| values.get(name))
                            && matches!(
                                mask,
                                Type::FixedVector { .. }
                                    | Type::ScalableVector(_)
                                    | Type::Tuple(_)
                                    | Type::Array { .. }
                                    | Type::Address { .. }
                            )
                        {
                            trailing_roots.push((mask.clone(), function.item));
                        }
                        if instruction.opcode == "addr.toptr"
                            && let Some(Type::Pointer(space)) = instruction
                                .results
                                .first()
                                .and_then(|name| values.get(name))
                        {
                            trailing_roots.push((Type::Pointer(*space), function.item));
                        }
                        if instruction.opcode == "enum.tag"
                            && let Some(ty) = instruction
                                .results
                                .first()
                                .and_then(|name| values.get(name))
                        {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                        if crate::text_managed_heap::handles(&instruction.opcode)
                            && let Some(ty) = instruction
                                .results
                                .first()
                                .and_then(|name| values.get(name))
                        {
                            trailing_roots.push((ty.clone(), function.item));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
