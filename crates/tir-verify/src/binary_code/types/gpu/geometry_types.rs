use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

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
        .filter(|instruction| matches!(instruction.opcode, 0x4c0..=0x4c4 | 0x5a0 | 0x5c0..=0x5c5))
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
        0x4c0 => {
            let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
                return;
            };
            for index in 0..3 {
                check_operand(
                    instruction,
                    values,
                    index,
                    value_types,
                    types,
                    is_integer,
                    "object.dispatch_mesh operands must be integers",
                    function_name,
                    diagnostics,
                );
            }
        }
        0x4c1 => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            for index in 0..2 {
                check_operand(
                    instruction,
                    values,
                    index,
                    value_types,
                    types,
                    is_integer,
                    "mesh.set_counts operands must be integers",
                    function_name,
                    diagnostics,
                );
            }
        }
        0x4c2 | 0x4c3 => {
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
                "mesh output index must be an integer",
                function_name,
                diagnostics,
            );
        }
        0x4c4 => {
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
                "mesh.write.indices offset must be an integer",
                function_name,
                diagnostics,
            );
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_index_value,
                "mesh.write.indices requires an i32 index or vector of i32 indices",
                function_name,
                diagnostics,
            );
        }
        0x5a0 => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_vec2_i32,
                "tile.imageblock.addr position must have type vec<2 x i32>",
                function_name,
                diagnostics,
            );
        }
        0x5c0 => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_integer,
                "node record index must be an integer",
                function_name,
                diagnostics,
            );
        }
        0x5c2 => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                1,
                value_types,
                types,
                is_integer,
                "node output count must be an integer",
                function_name,
                diagnostics,
            );
        }
        0x5c3 => {
            let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_node_record_pointer,
                "node.output.addr allocation must have type ptr<node_record>",
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
                "node record index must be an integer",
                function_name,
                diagnostics,
            );
        }
        0x5c4 => {
            let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
                return;
            };
            check_operand(
                instruction,
                values,
                0,
                value_types,
                types,
                is_node_record_pointer,
                "node.output.commit allocation must have type ptr<node_record>",
                function_name,
                diagnostics,
            );
        }
        _ => {}
    }
}

fn require_values<'a>(
    instruction: &'a BinaryInstruction,
    expected: usize,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [u64]> {
    let values = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.as_slice()),
            _ => None,
        });
    if values.is_none_or(|values| values.len() != expected) {
        report(
            diagnostics,
            function_name,
            instruction,
            "GPU geometry instruction has an invalid number of value operands",
        );
        return None;
    }
    values
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

fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(_)))
}

fn is_i32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(32)))
}

fn is_index_value(types: &BinaryTypeTable, index: u64) -> bool {
    match type_at(types, index) {
        Some(BinaryType::Integer(32)) => true,
        Some(BinaryType::FixedVector {
            lanes: 2 | 3,
            element,
        }) => is_i32(types, *element),
        _ => false,
    }
}

fn is_vec2_i32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::FixedVector { lanes: 2, element }) if is_i32(types, *element)
    )
}

fn is_node_record_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(9)))
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
