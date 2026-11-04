use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, TypeSection};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        match instruction.opcode {
            0x703 => {
                verify_value_type(
                    instruction,
                    0,
                    value_types,
                    types,
                    is_scalar_integer,
                    "stack.alloc.dyn size must be a scalar integer",
                    &mut diagnostics,
                );
                let alignment = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Index(value) => Some(*value),
                        _ => None,
                    });
                if !alignment
                    .is_some_and(|value| (1..=4096).contains(&value) && value.is_power_of_two())
                {
                    diagnostics.push(Diagnostic::error(
                        VFY3,
                        "stack.alloc.dyn alignment must be a power of two from 1 through 4096 bytes",
                    ));
                }
            }
            0x705 => verify_value_type(
                instruction,
                0,
                value_types,
                types,
                is_pointer,
                "stack.restore state must have pointer type",
                &mut diagnostics,
            ),
            0x707 => verify_value_type(
                instruction,
                0,
                value_types,
                types,
                is_i1,
                "trap.if condition must have type i1",
                &mut diagnostics,
            ),
            0x708 => verify_value_type(
                instruction,
                0,
                value_types,
                types,
                is_pointer,
                "code.load address must have pointer type",
                &mut diagnostics,
            ),
            0x709 => {
                verify_value_type(
                    instruction,
                    0,
                    value_types,
                    types,
                    is_code,
                    "code.store value must have type code",
                    &mut diagnostics,
                );
                verify_value_type(
                    instruction,
                    1,
                    value_types,
                    types,
                    is_pointer,
                    "code.store address must have pointer type",
                    &mut diagnostics,
                );
            }
            0x70a => verify_value_type(
                instruction,
                0,
                value_types,
                types,
                is_code,
                "code.addr operand must have type code",
                &mut diagnostics,
            ),
            0x70b => verify_value_type(
                instruction,
                0,
                value_types,
                types,
                is_pointer,
                "code.fromaddr operand must have pointer type",
                &mut diagnostics,
            ),
            _ => {}
        }
    }
    diagnostics
}

fn verify_value_type(
    instruction: &BinaryInstruction,
    value_index: usize,
    value_types: &[Option<u64>],
    types: &TypeSection,
    predicate: fn(&tvisa_format::TvisaValueTypeRecord) -> bool,
    message: &'static str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let value = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .nth(value_index);
    let actual = value
        .and_then(|value| usize::try_from(*value).ok())
        .and_then(|value| value_types.get(value))
        .copied()
        .flatten()
        .and_then(|type_index| types.value_types.get(type_index as usize));
    if actual.is_some_and(|actual| !predicate(actual)) {
        diagnostics.push(Diagnostic::error(VFY3, message));
    }
}

fn is_scalar_integer(ty: &tvisa_format::TvisaValueTypeRecord) -> bool {
    matches!(ty, tvisa_format::TvisaValueTypeRecord::Integer { .. })
}

fn is_i1(ty: &tvisa_format::TvisaValueTypeRecord) -> bool {
    matches!(ty, tvisa_format::TvisaValueTypeRecord::Integer { bits: 1 })
}

fn is_pointer(ty: &tvisa_format::TvisaValueTypeRecord) -> bool {
    matches!(ty, tvisa_format::TvisaValueTypeRecord::Pointer { .. })
}

fn is_code(ty: &tvisa_format::TvisaValueTypeRecord) -> bool {
    matches!(ty, tvisa_format::TvisaValueTypeRecord::Code)
}
