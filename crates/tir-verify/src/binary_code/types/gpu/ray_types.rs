use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
        .filter(|instruction| (0x500..=0x511).contains(&instruction.opcode))
    {
        verify_instruction(
            instruction,
            value_types,
            types,
            function_name,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match instruction.opcode {
        0x500 => {
            let Some(values) = require_values(instruction, 8, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_thread_pointer,
                "rq.init query must be a ptr<thread>",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_acceleration_structure,
                "rq.init requires an acceleration structure",
                function_name,
                diagnostics,
            );
            for index in [2, 3] {
                check_operand(
                    instruction,
                    values,
                    index,
                    value_types,
                    types,
                    is_integer,
                    "rq.init flags and mask must be integers",
                    function_name,
                    diagnostics,
                );
            }
            for index in [4, 6] {
                check_operand(
                    instruction,
                    values,
                    index,
                    value_types,
                    types,
                    is_vec3_f32,
                    "rq.init origin and direction must have type vec<3 x f32>",
                    function_name,
                    diagnostics,
                );
            }
            for index in [5, 7] {
                check_operand(
                    instruction,
                    values,
                    index,
                    value_types,
                    types,
                    is_f32,
                    "rq.init tmin and tmax must have type f32",
                    function_name,
                    diagnostics,
                );
            }
        }
        0x501 | 0x502 | 0x504 | 0x505 => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_thread_pointer,
                "ray-query operand must be a ptr<thread>",
                function_name,
                diagnostics,
            );
        }
        0x503 => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_thread_pointer,
                "rq.commit_procedural query must be a ptr<thread>",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_f32,
                "rq.commit_procedural t must have type f32",
                function_name,
                diagnostics,
            );
        }
        0x506 | 0x50c => {
            let expected = if instruction.opcode == 0x506 { 11 } else { 10 };
            let Some(values) = require_values(instruction, expected, function_name, diagnostics)
            else {
                return;
            };
            verify_trace_operands(
                instruction,
                values,
                value_types,
                types,
                function_name,
                diagnostics,
            );
            if instruction.opcode == 0x506 {
                check_operand(
                    instruction,
                    values,
                    10,
                    value_types,
                    types,
                    is_ray_payload_pointer,
                    "rt.trace payload must have type ptr<ray_payload>",
                    function_name,
                    diagnostics,
                );
            }
        }
        0x507 => {
            let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_f32,
                "rt.report_hit t must have type f32",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_integer,
                "rt.report_hit kind must be an integer",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                2,
                value_types,
                types,
                is_hit_attribute_pointer,
                "rt.report_hit attributes must have type ptr<hit_attribute>",
                function_name,
                diagnostics,
            );
        }
        0x50a => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_integer,
                "rt.call_callable index must be an integer",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_pointer,
                "rt.call_callable data must be a pointer",
                function_name,
                diagnostics,
            );
        }
        0x50d => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_thread_pointer,
                "rt.hitobject.from_query requires a ptr<thread>",
                function_name,
                diagnostics,
            );
        }
        0x50f => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_hit_object,
                "rt.hitobject.invoke requires a hitobject",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_ray_payload_pointer,
                "rt.hitobject.invoke payload must have type ptr<ray_payload>",
                function_name,
                diagnostics,
            );
        }
        0x510 => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_hit_object,
                "rt.hitobject.get requires a hitobject",
                function_name,
                diagnostics,
            );
        }
        0x511 => verify_reorder(instruction, value_types, types, function_name, diagnostics),
        _ => {}
    }
}

fn verify_trace_operands(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode;
    check_operand(
        instruction,
        values,
        0,
        value_types,
        types,
        is_acceleration_structure,
        if opcode == 0x506 {
            "rt.trace requires an acceleration structure"
        } else {
            "rt.hitobject.trace requires an acceleration structure"
        },
        function_name,
        diagnostics,
    );
    for index in 1..=5 {
        check_operand(
            instruction,
            values,
            index,
            value_types,
            types,
            is_integer,
            if opcode == 0x506 {
                "rt.trace flags, mask, SBT values, and miss index must be integers"
            } else {
                "rt.hitobject.trace flags, mask, SBT values, and miss index must be integers"
            },
            function_name,
            diagnostics,
        );
    }
    for index in [6, 8] {
        check_operand(
            instruction,
            values,
            index,
            value_types,
            types,
            is_vec3_f32,
            "ray trace origin and direction must have type vec<3 x f32>",
            function_name,
            diagnostics,
        );
    }
    for index in [7, 9] {
        check_operand(
            instruction,
            values,
            index,
            value_types,
            types,
            is_f32,
            "ray trace tmin and tmax must have type f32",
            function_name,
            diagnostics,
        );
    }
}

fn verify_reorder(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = values(instruction) else {
        report(
            diagnostics,
            function_name,
            instruction,
            "rt.reorder requires a hitobject and optional key",
        );
        return;
    };
    if !matches!(values.len(), 1 | 2) {
        report(
            diagnostics,
            function_name,
            instruction,
            "rt.reorder requires a hitobject and optional key",
        );
        return;
    }
    check_operand(
        instruction,
        values,
        0,
        value_types,
        types,
        is_hit_object,
        "rt.reorder requires a hitobject",
        function_name,
        diagnostics,
    );
    if values.len() == 2 {
        check_operand(
            instruction,
            values,
            1,
            value_types,
            types,
            is_integer,
            "rt.reorder hint must be an integer",
            function_name,
            diagnostics,
        );
    }
}

fn require_values<'a>(
    instruction: &'a BinaryInstruction,
    expected: usize,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [u64]> {
    let Some(values) = values(instruction) else {
        report(
            diagnostics,
            function_name,
            instruction,
            "ray instruction has no value operands",
        );
        return None;
    };
    if values.len() != expected {
        report(
            diagnostics,
            function_name,
            instruction,
            "ray instruction has an invalid number of value operands",
        );
        return None;
    }
    Some(values)
}

fn values(instruction: &BinaryInstruction) -> Option<&[u64]> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.as_slice()),
            _ => None,
        })
}

#[allow(clippy::too_many_arguments)]
fn check_operand(
    instruction: &BinaryInstruction,
    values: &[u64],
    operand_index: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    predicate: fn(&BinaryTypeTable, u64) -> bool,
    message: &str,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if values
        .get(operand_index)
        .and_then(|value| value_type(*value, value_types))
        .is_some_and(|actual| !predicate(types, actual))
    {
        report(diagnostics, function_name, instruction, message);
    }
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
}

fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(_)))
}

fn is_f32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::Float(FloatType::F32))
    )
}

fn is_vec3_f32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::FixedVector { lanes: 3, element }) if is_f32(types, *element)
    )
}

fn is_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(_)))
}

fn is_thread_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(4)))
}

fn is_ray_payload_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(6)))
}

fn is_hit_attribute_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(7)))
}

fn is_acceleration_structure(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::AccelerationStructure)
    )
}

fn is_hit_object(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::HitObject))
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    instruction: &BinaryInstruction,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function_name} CODE offset {}: {message}",
            instruction.offset
        ),
    ));
}
