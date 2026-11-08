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
        .filter(|instruction| matches!(instruction.opcode, 0x420..=0x426 | 0x440..=0x452))
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
    let expected = match instruction.opcode {
        0x420..=0x422 | 0x424..=0x426 | 0x441..=0x443 | 0x445..=0x44a | 0x450..=0x452 => 1,
        0x423 | 0x444 | 0x44b..=0x44f => 2,
        0x440 => 0,
        _ => return,
    };
    let values = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(vec![*value]),
            BinaryOperand::Values(values) => Some(values.clone()),
            _ => None,
        })
        .or_else(|| (expected == 0).then(Vec::new));
    let Some(values) = values.filter(|values| values.len() == expected) else {
        report(
            diagnostics,
            function_name,
            instruction,
            "GPU collective instruction has an invalid number of value operands",
        );
        return;
    };
    let operand_type = |index: usize| {
        values
            .get(index)
            .and_then(|value| value_type(*value, value_types))
            .and_then(|ty| type_at(types, ty))
    };
    match instruction.opcode {
        0x420..=0x422 if !operand_type(0).is_some_and(|ty| is_float_value(ty, types)) => report(
            diagnostics,
            function_name,
            instruction,
            "GPU derivative operand must be a floating-point scalar or vector",
        ),
        0x423 if !operand_type(1).is_some_and(is_i32) => report(
            diagnostics,
            function_name,
            instruction,
            "quad.broadcast lane must have type i32",
        ),
        0x425 | 0x426 | 0x441 | 0x442 | 0x446 if !operand_type(0).is_some_and(is_i1) => {
            report(
                diagnostics,
                function_name,
                instruction,
                "quad and subgroup predicates require an i1 operand",
            );
        }
        0x443 if !operand_type(0).is_some_and(is_scalar_value) => report(
            diagnostics,
            function_name,
            instruction,
            "subgroup.all_equal requires a scalar value",
        ),
        0x444 | 0x44b..=0x44f if !operand_type(1).is_some_and(is_i32) => report(
            diagnostics,
            function_name,
            instruction,
            "subgroup lane or offset operand must have type i32",
        ),
        0x447..=0x44a if !operand_type(0).is_some_and(|ty| is_ballot(ty, types)) => report(
            diagnostics,
            function_name,
            instruction,
            "subgroup ballot query requires vec<4 x i32>",
        ),
        0x450..=0x452 => verify_reduction(instruction, operand_type(0), function_name, diagnostics),
        _ => {}
    }
}

fn verify_reduction(
    instruction: &BinaryInstruction,
    ty: Option<&BinaryType>,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let indices = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some((&operator, &cluster)) = indices.first().zip(indices.get(1)) else {
        report(
            diagnostics,
            function_name,
            instruction,
            "subgroup reduction is missing its operator or cluster field",
        );
        return;
    };
    if cluster != 0 && (cluster > 128 || !cluster.is_power_of_two()) {
        report(
            diagnostics,
            function_name,
            instruction,
            "subgroup cluster size must be a power of two no greater than 128",
        );
    }
    let valid = match ty {
        Some(BinaryType::Integer(_)) => {
            operator <= 1
                || (3..=4).contains(&operator)
                || (6..=7).contains(&operator)
                || (9..=10).contains(&operator)
                || (12..=14).contains(&operator)
        }
        Some(BinaryType::Float(_)) => matches!(operator, 2 | 5 | 8 | 11),
        _ => false,
    };
    if !valid {
        report(
            diagnostics,
            function_name,
            instruction,
            "subgroup reduction operator is incompatible with its scalar operand",
        );
    }
}

fn is_i1(ty: &BinaryType) -> bool {
    matches!(ty, BinaryType::Integer(1))
}

fn is_i32(ty: &BinaryType) -> bool {
    matches!(ty, BinaryType::Integer(32))
}

fn is_scalar_value(ty: &BinaryType) -> bool {
    matches!(ty, BinaryType::Integer(_) | BinaryType::Float(_))
}

fn is_float_value(ty: &BinaryType, types: &BinaryTypeTable) -> bool {
    match ty {
        BinaryType::Float(_) => true,
        BinaryType::FixedVector { element, .. } => {
            matches!(type_at(types, *element), Some(BinaryType::Float(_)))
        }
        _ => false,
    }
}

fn is_ballot(ty: &BinaryType, types: &BinaryTypeTable) -> bool {
    matches!(
        ty,
        BinaryType::FixedVector { lanes: 4, element }
            if matches!(type_at(types, *element), Some(BinaryType::Integer(32)))
    )
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
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
            "function @{function_name} at CODE+0x{:x}: {message}",
            instruction.offset
        ),
    ));
}
