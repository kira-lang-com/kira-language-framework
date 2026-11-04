use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryEntryKind, BinaryFunctionBody, BinaryType, BinaryTypeTable, StringTable};
use tir_types::FloatType;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify(
    stage: BinaryEntryKind,
    body: &BinaryFunctionBody,
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(strings) = strings else {
        return;
    };
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter(|instruction| instruction.opcode == 0x400)
    {
        let name = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                tir_format::BinaryOperand::Index(index) => usize::try_from(*index)
                    .ok()
                    .and_then(|index| strings.get(index)),
                _ => None,
            });
        let Some(name) = name else {
            continue;
        };
        let Some(expected) = builtin_type(name) else {
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "function @{function_name} CODE offset {}: unknown GPU builtin '{name}'",
                    instruction.offset
                ),
            ));
            continue;
        };
        let actual = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                tir_format::BinaryOperand::Type(index) => Some(*index),
                _ => None,
            });
        if actual.is_some_and(|actual| !matches_type(types, actual, expected)) {
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "function @{function_name} CODE offset {}: GPU builtin '{name}' has an invalid result type",
                    instruction.offset
                ),
            ));
        }
        if !available_in(name, stage) {
            diagnostics.push(Diagnostic::error(
                V15,
                format!(
                    "function @{function_name} CODE offset {}: GPU builtin '{name}' is not available in the {} stage",
                    instruction.offset,
                    stage_name(stage)
                ),
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum BuiltinType {
    I1,
    I32,
    I32x2,
    I32x3,
    F32x2,
    F32x3,
    F32x4,
}

fn builtin_type(name: &str) -> Option<BuiltinType> {
    Some(match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "threadgroups_per_grid" => BuiltinType::I32x3,
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
        | "viewport_index" => BuiltinType::I32,
        "position" => BuiltinType::F32x4,
        "front_facing" | "helper_invocation" => BuiltinType::I1,
        "barycentric_perspective" | "barycentric_linear" => BuiltinType::F32x3,
        "point_coord" => BuiltinType::F32x2,
        "shading_rate" => BuiltinType::I32x2,
        _ => return None,
    })
}

fn matches_type(types: &BinaryTypeTable, index: u64, expected: BuiltinType) -> bool {
    let Some(actual) = usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    match expected {
        BuiltinType::I1 => matches!(actual, BinaryType::Integer(1)),
        BuiltinType::I32 => matches!(actual, BinaryType::Integer(32)),
        BuiltinType::I32x2 => vector_is(types, actual, 2, ElementType::Integer(32)),
        BuiltinType::I32x3 => vector_is(types, actual, 3, ElementType::Integer(32)),
        BuiltinType::F32x2 => vector_is(types, actual, 2, ElementType::Float(FloatType::F32)),
        BuiltinType::F32x3 => vector_is(types, actual, 3, ElementType::Float(FloatType::F32)),
        BuiltinType::F32x4 => vector_is(types, actual, 4, ElementType::Float(FloatType::F32)),
    }
}

#[derive(Clone, Copy)]
enum ElementType {
    Integer(u64),
    Float(FloatType),
}

fn vector_is(
    types: &BinaryTypeTable,
    actual: &BinaryType,
    expected_lanes: u64,
    expected_element: ElementType,
) -> bool {
    let BinaryType::FixedVector { lanes, element } = actual else {
        return false;
    };
    if *lanes != expected_lanes {
        return false;
    }
    match (
        types
            .entries
            .get(usize::try_from(*element).unwrap_or(usize::MAX)),
        expected_element,
    ) {
        (Some(BinaryType::Integer(actual)), ElementType::Integer(expected)) => *actual == expected,
        (Some(BinaryType::Float(actual)), ElementType::Float(expected)) => *actual == expected,
        _ => false,
    }
}

fn available_in(name: &str, stage: BinaryEntryKind) -> bool {
    match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "thread_index_in_threadgroup"
        | "subgroup_index_in_threadgroup"
        | "subgroups_per_threadgroup" => matches!(
            stage,
            BinaryEntryKind::Compute
                | BinaryEntryKind::Object
                | BinaryEntryKind::Mesh
                | BinaryEntryKind::Tile
        ),
        "threadgroups_per_grid" => matches!(
            stage,
            BinaryEntryKind::Compute | BinaryEntryKind::Object | BinaryEntryKind::Mesh
        ),
        "lane_index" | "subgroup_size" => true,
        "vertex_id" | "instance_id" | "base_vertex" | "base_instance" | "draw_index" => {
            stage == BinaryEntryKind::Vertex
        }
        "view_index" => matches!(
            stage,
            BinaryEntryKind::Vertex | BinaryEntryKind::Mesh | BinaryEntryKind::Fragment
        ),
        "position"
        | "front_facing"
        | "sample_index"
        | "sample_mask_in"
        | "barycentric_perspective"
        | "barycentric_linear"
        | "primitive_id"
        | "layer"
        | "viewport_index"
        | "point_coord"
        | "helper_invocation"
        | "shading_rate" => stage == BinaryEntryKind::Fragment,
        _ => false,
    }
}

fn stage_name(stage: BinaryEntryKind) -> &'static str {
    match stage {
        BinaryEntryKind::Compute => "compute",
        BinaryEntryKind::Object => "object",
        BinaryEntryKind::Mesh => "mesh",
        BinaryEntryKind::Vertex => "vertex",
        BinaryEntryKind::Fragment => "fragment",
        BinaryEntryKind::Tile => "tile",
        BinaryEntryKind::Raygen => "raygen",
        BinaryEntryKind::Intersection => "intersection",
        BinaryEntryKind::Anyhit => "anyhit",
        BinaryEntryKind::Closesthit => "closesthit",
        BinaryEntryKind::Miss => "miss",
        BinaryEntryKind::Callable => "callable",
        BinaryEntryKind::Node => "node",
    }
}
