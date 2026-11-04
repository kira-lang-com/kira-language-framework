use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

use super::selector;

mod helpers;
use helpers::*;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

#[derive(Clone, Copy)]
pub(super) struct Matrix {
    pub(super) scope: u8,
    pub(super) rows: u64,
    pub(super) columns: u64,
    pub(super) element: u64,
    pub(super) use_kind: u8,
}

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
        .filter(|instruction| (0x560..=0x56b).contains(&instruction.opcode))
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
        0x560 => verify_fill(instruction, value_types, types, function_name, diagnostics),
        0x561 => verify_load(instruction, value_types, types, function_name, diagnostics),
        0x562 => verify_store(instruction, value_types, types, function_name, diagnostics),
        0x563 => verify_mma(instruction, value_types, types, function_name, diagnostics),
        0x564 => verify_convert(instruction, value_types, types, function_name, diagnostics),
        0x565 => verify_length(instruction, types, function_name, diagnostics),
        0x566 => verify_extract(instruction, value_types, types, function_name, diagnostics),
        0x567 => verify_insert(instruction, value_types, types, function_name, diagnostics),
        0x568 => verify_reduce(instruction, value_types, types, function_name, diagnostics),
        0x569 => verify_tensor_load(instruction, value_types, types, function_name, diagnostics),
        0x56a => verify_tensor_store(instruction, value_types, types, function_name, diagnostics),
        0x56b => verify_tensor_query(instruction, value_types, types, function_name, diagnostics),
        _ => {}
    }
}

fn verify_fill(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
        return;
    };
    let Some(matrix) = declared_matrix(instruction, types, function_name, diagnostics) else {
        return;
    };
    check_value_type(
        instruction,
        values,
        0,
        value_types,
        types,
        |types, actual| same_type(types, actual, matrix.element),
        "coop.fill value must match the cooperative matrix element type",
        function_name,
        diagnostics,
    );
}

fn verify_load(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
        return;
    };
    let Some(matrix) = declared_matrix(instruction, types, function_name, diagnostics) else {
        return;
    };
    let Some(source) = value_type(values[0], value_types) else {
        return;
    };
    match type_at(types, source) {
        Some(BinaryType::Tensor { rank, element }) => {
            if !same_type(types, *element, matrix.element) {
                report(
                    diagnostics,
                    function_name,
                    instruction,
                    "coop.load tensor element type must match the matrix element type",
                );
            }
            check_value_type(
                instruction,
                values,
                1,
                value_types,
                types,
                |types, actual| tensor_coordinates(types, actual, *rank),
                "coop.load coordinates do not match the tensor rank",
                function_name,
                diagnostics,
            );
        }
        Some(BinaryType::Pointer(1 | 3)) => check_value_type(
            instruction,
            values,
            1,
            value_types,
            types,
            is_integer,
            "coop.load stride must be an integer",
            function_name,
            diagnostics,
        ),
        Some(_) => report(
            diagnostics,
            function_name,
            instruction,
            "coop.load requires a tensor or a device/threadgroup pointer",
        ),
        None => {}
    }
}

fn verify_store(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
        return;
    };
    let Some(matrix) = matrix_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    check_value_type(
        instruction,
        values,
        1,
        value_types,
        types,
        is_memory_pointer,
        "cooperative memory operand must be a device or threadgroup pointer",
        function_name,
        diagnostics,
    );
    check_value_type(
        instruction,
        values,
        2,
        value_types,
        types,
        is_integer,
        "coop.store stride must be an integer",
        function_name,
        diagnostics,
    );
    let _ = matrix;
}

fn verify_mma(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
        return;
    };
    let matrices = [0, 1, 2].map(|index| {
        matrix_for_value(
            instruction,
            values,
            index,
            value_types,
            types,
            function_name,
            diagnostics,
        )
    });
    let [Some(a), Some(b), Some(c)] = matrices else {
        return;
    };
    if (a.use_kind, b.use_kind, c.use_kind) != (0, 1, 2) {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma operands must use a, b, and acc roles",
        );
    }
    if a.scope != b.scope || a.scope != c.scope {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma operands must have the same scope",
        );
    }
    if a.columns != b.rows || a.rows != c.rows || b.columns != c.columns {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma matrix dimensions are incompatible",
        );
    }
    if !same_type(types, a.element, b.element) {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma input matrices must have the same element type",
        );
    }
    let unsigned = instruction.modifiers & (1 << 2) != 0;
    let saturating = instruction.modifiers & (1 << 46) != 0;
    if unsigned && !is_integer(types, a.element) {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma unsigned requires integer inputs",
        );
    }
    if saturating && !is_integer(types, c.element) {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.mma saturate requires an integer accumulator",
        );
    }
}

fn verify_convert(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
        return;
    };
    let Some(source) = matrix_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    let Some(target) = instruction_type(instruction).and_then(|index| matrix_at(types, index))
    else {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.convert target must be a cooperative matrix",
        );
        return;
    };
    if (source.scope, source.rows, source.columns) != (target.scope, target.rows, target.columns) {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.convert must preserve matrix scope and dimensions",
        );
    }
}

fn verify_length(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if instruction_type(instruction)
        .and_then(|index| matrix_at(types, index))
        .is_none()
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.length requires a cooperative matrix type",
        );
    }
}

fn verify_extract(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
        return;
    };
    matrix_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    );
    check_value_type(
        instruction,
        values,
        1,
        value_types,
        types,
        is_integer,
        "coop.extract index must be an integer",
        function_name,
        diagnostics,
    );
}

fn verify_insert(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
        return;
    };
    let Some(matrix) = matrix_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    check_value_type(
        instruction,
        values,
        1,
        value_types,
        types,
        is_integer,
        "coop.insert index must be an integer",
        function_name,
        diagnostics,
    );
    check_value_type(
        instruction,
        values,
        2,
        value_types,
        types,
        |types, actual| same_type(types, actual, matrix.element),
        "coop.insert value must match the cooperative matrix element type",
        function_name,
        diagnostics,
    );
}

fn verify_reduce(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
        return;
    };
    let Some(matrix) = matrix_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    let operation = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let integer_operation = matches!(operation, Some(0 | 1 | 3 | 4 | 6 | 7 | 9 | 10 | 12..=14));
    let float_operation = matches!(operation, Some(2 | 5 | 8 | 11));
    if !(integer_operation && is_integer(types, matrix.element)
        || float_operation && is_float(types, matrix.element))
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.reduce operator is missing or incompatible with the matrix element type",
        );
    }
    if selector(instruction) == 2
        && (matrix.scope != 0 || matrix.rows % 2 != 0 || matrix.columns % 2 != 0)
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "coop.reduce quad2x2 requires an even subgroup matrix shape",
        );
    }
}

fn verify_tensor_load(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 2, function_name, diagnostics) else {
        return;
    };
    let Some((rank, _)) = tensor_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    check_value_type(
        instruction,
        values,
        1,
        value_types,
        types,
        |types, actual| tensor_coordinates(types, actual, rank),
        "tensor.load coordinates do not match the tensor rank",
        function_name,
        diagnostics,
    );
}

fn verify_tensor_store(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 3, function_name, diagnostics) else {
        return;
    };
    let Some((rank, element)) = tensor_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    check_value_type(
        instruction,
        values,
        1,
        value_types,
        types,
        |types, actual| tensor_coordinates(types, actual, rank),
        "tensor.store coordinates do not match the tensor rank",
        function_name,
        diagnostics,
    );
    check_value_type(
        instruction,
        values,
        2,
        value_types,
        types,
        |types, actual| same_type(types, actual, element),
        "tensor.store value must match the tensor element type",
        function_name,
        diagnostics,
    );
}

fn verify_tensor_query(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = require_values(instruction, 1, function_name, diagnostics) else {
        return;
    };
    let Some((rank, _)) = tensor_for_value(
        instruction,
        values,
        0,
        value_types,
        types,
        function_name,
        diagnostics,
    ) else {
        return;
    };
    let dimension = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Integer(value) if !value.negative => Some(value.magnitude),
            _ => None,
        });
    if dimension.is_none_or(|dimension| dimension >= u128::from(rank)) {
        report(
            diagnostics,
            function_name,
            instruction,
            "tensor.query dimension must be less than the tensor rank",
        );
    }
}
