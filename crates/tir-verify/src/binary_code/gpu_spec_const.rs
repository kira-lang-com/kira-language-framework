//! V2 GPU specialization defaults: 32-bit IDs and typed CONSTANTS references.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryConstantTable, BinaryConstantValue, BinaryFunctionBody, BinaryInstruction, BinaryOperand,
    BinaryTarget, BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    types: &BinaryTypeTable,
    constants: Option<&BinaryConstantTable>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut issues = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if instruction.opcode != 0x401 {
            continue;
        }
        if let Some(message) = check(instruction, target, types, constants) {
            issues.push(Diagnostic::error(
                V2,
                format!(
                    "function @{function_name} CODE offset {}: {message}",
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
    constants: Option<&BinaryConstantTable>,
) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("gpu.spec_const requires target(gpu)");
    }
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Some("gpu.spec_const requires zero modifiers and exactly one result");
    }
    let [
        BinaryOperand::Index(id),
        BinaryOperand::Type(ty),
        BinaryOperand::Index(index),
    ] = instruction.operands.as_slice()
    else {
        return Some("gpu.spec_const requires id, TYPES index and CONSTANTS index");
    };
    if u32::try_from(*id).is_err() {
        return Some("gpu.spec_const ID must fit u32");
    }
    let Some(record) = constants.and_then(|constants| {
        usize::try_from(*index)
            .ok()
            .and_then(|index| constants.records.get(index))
    }) else {
        return Some("gpu.spec_const CONSTANTS index is missing or out of bounds");
    };
    if record.ty != *ty {
        return Some("gpu.spec_const CONSTANTS type does not match the CODE result type");
    }
    if !valid_value(types, *ty, &record.value) {
        return Some("gpu.spec_const has unsupported GPU type or invalid typed default");
    }
    None
}

fn valid_value(types: &BinaryTypeTable, ty: u64, value: &BinaryConstantValue) -> bool {
    let Some(ty) = usize::try_from(ty)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    match (ty, value) {
        (BinaryType::Integer(bits @ 1..=64), BinaryConstantValue::Integer(bytes)) => {
            let width = (*bits as usize).div_ceil(8);
            bytes.len() == width
                && (bits % 8 == 0 || bytes.last().is_some_and(|last| last >> (bits % 8) == 0))
        }
        (
            BinaryType::Float(
                float @ (FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64),
            ),
            BinaryConstantValue::Float(bytes),
        ) => bytes.len() == usize::from(float.bits() / 8),
        (
            BinaryType::FixedVector {
                lanes: lanes @ 2..=4,
                element,
            },
            BinaryConstantValue::Vector(values),
        ) => {
            values.len() == *lanes as usize
                && values
                    .iter()
                    .all(|value| valid_scalar(types, *element, value))
        }
        _ => false,
    }
}

fn valid_scalar(types: &BinaryTypeTable, ty: u64, value: &BinaryConstantValue) -> bool {
    matches!(
        types.entries.get(ty as usize),
        Some(
            BinaryType::Integer(1..=64)
                | BinaryType::Float(
                    FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64
                )
        )
    ) && valid_value(types, ty, value)
}
