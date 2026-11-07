//! GPU builtin CODE (0x400): named, typechecked, STRINGS-backed SSA values.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryTypeTable};
use crate::{FormatError, StringTable, TextInstruction, TextItem, TextParseError};

#[derive(Debug)]
pub(crate) struct GpuBuiltin {
    pub(crate) name: String,
    pub(crate) result: String,
    pub(crate) ty: Type,
}

/// The normative section 17.5 names and result types, shared with V2 verification.
#[must_use]
pub fn type_for_name(name: &str) -> Option<Type> {
    let i1 = Type::Integer(1);
    let i32 = Type::Integer(32);
    let f32 = Type::Float(FloatType::F32);
    let vector = |lanes, element| Type::FixedVector {
        lanes,
        element: Box::new(element),
    };
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

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<GpuBuiltin, TextParseError> {
    if instruction.opcode != "gpu.builtin" {
        return Err(error(item, instruction.span, "expected gpu.builtin"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "gpu.builtin requires one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "gpu.builtin source span is invalid"))?
        .trim();
    let prefix = format!("%{result} = gpu.builtin ");
    let (name, declared) = line
        .strip_prefix(&prefix)
        .and_then(|text| text.split_once(" : "))
        .ok_or_else(|| error(item, instruction.span, "gpu.builtin requires name : T"))?;
    let ty = crate::parse_type(declared)
        .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    let expected = type_for_name(name)
        .ok_or_else(|| error(item, instruction.span, "unknown GPU builtin name"))?;
    if ty != expected {
        return Err(error(
            item,
            instruction.span,
            format!("GPU builtin {name} must have type {expected}"),
        ));
    }
    Ok(GpuBuiltin {
        name: name.to_owned(),
        result: result.clone(),
        ty,
    })
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let operation = parse(instruction, source, item)?;
    if values.insert(operation.result, operation.ty).is_some() {
        return Err(error(
            item,
            instruction.span,
            "gpu.builtin result SSA name is duplicated",
        ));
    }
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    string_indices: &HashMap<usize, u64>,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let builtin = parse(instruction, source, item)?;
    let name = string_indices
        .get(&instruction.span.start)
        .copied()
        .filter(|index| *index > 0)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "GPU builtin name not interned in STRINGS",
            )
        })?;
    let ty = crate::text_descriptor_reference::type_index(types, &builtin.ty).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "GPU builtin result type not interned in TYPES",
        )
    })?;
    for value in [0x400, 0, 1, name, ty as u64] {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    strings: &StringTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if operation.opcode != 0x400 || operation.modifiers != 0 || operation.result_count != 1 {
        return Err(malformed(
            "gpu.builtin requires one SSA result and no modifiers",
        ));
    }
    let [BinaryOperand::Index(name_index), BinaryOperand::Type(ty)] = operation.operands.as_slice()
    else {
        return Err(malformed("gpu.builtin requires string and type indexes"));
    };
    let name = usize::try_from(*name_index)
        .ok()
        .and_then(|index| strings.get(index))
        .ok_or_else(|| malformed("gpu.builtin name string index is out of range"))?;
    let expected = type_for_name(name).ok_or_else(|| malformed("unknown GPU builtin name"))?;
    if crate::text_descriptor_reference::type_index(types, &expected) != usize::try_from(*ty).ok() {
        return Err(malformed(
            "gpu.builtin declared type differs from normative type",
        ));
    }
    let ty_name = crate::text_type_disassembly::format_type_reference(
        types,
        *ty,
        &crate::types::BinarySignatureTable::default(),
    )?;
    let result = format!("v{}", values.len());
    writeln!(text, "    %{result} = gpu.builtin {name} : {ty_name}")
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((result, *ty));
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
