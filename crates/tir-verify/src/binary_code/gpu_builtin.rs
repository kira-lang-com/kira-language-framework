//! V2 checks of GPU builtin names and their declared CODE result types.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget, BinaryType,
    BinaryTypeTable, StringTable,
};
use tir_types::Type;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut issues = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if instruction.opcode != 0x400 {
            continue;
        }
        let error = check(instruction, target, types, strings);
        if let Some(error) = error {
            issues.push(Diagnostic::error(
                V2,
                format!(
                    "function @{function_name} CODE offset {}: {error}",
                    instruction.offset
                ),
            ));
        }
    }
    issues
}

fn check(
    instruction: &BinaryInstruction,
    target: Option<BinaryTarget>,
    types: &BinaryTypeTable,
    strings: Option<&StringTable>,
) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("gpu.builtin is only valid in target(gpu) code");
    }
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Some("gpu.builtin must have zero modifiers and exactly one SSA result");
    }
    let [BinaryOperand::Index(name), BinaryOperand::Type(result_ty)] =
        instruction.operands.as_slice()
    else {
        return Some("gpu.builtin requires a STRINGS name and TYPES result index");
    };
    let Some(name) = strings.and_then(|strings| {
        usize::try_from(*name)
            .ok()
            .and_then(|index| strings.get(index))
    }) else {
        return Some("gpu.builtin STRINGS name index is missing or invalid");
    };
    let Some(expected) = tir_format::gpu_builtin_type(name) else {
        return Some("gpu.builtin has an unrecognized builtin name");
    };
    if !matches_type(types, *result_ty, &expected) {
        return Some("gpu.builtin name and declared TYPES result type disagree");
    }
    None
}

fn matches_type(types: &BinaryTypeTable, index: u64, expected: &Type) -> bool {
    match (
        usize::try_from(index)
            .ok()
            .and_then(|i| types.entries.get(i)),
        expected,
    ) {
        (Some(BinaryType::Integer(actual)), Type::Integer(expected)) => {
            *actual == u64::from(*expected)
        }
        (Some(BinaryType::Float(actual)), Type::Float(expected)) => actual == expected,
        (
            Some(BinaryType::FixedVector {
                lanes: actual,
                element: actual_ty,
            }),
            Type::FixedVector {
                lanes: expected,
                element,
            },
        ) => *actual == u64::from(*expected) && matches_type(types, *actual_ty, element),
        _ => false,
    }
}
