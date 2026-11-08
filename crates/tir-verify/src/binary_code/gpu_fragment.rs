use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if !matches!(instruction.opcode, 0x405..=0x409) {
            continue;
        }
        if let Some(message) = check(instruction, target) {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "function @{function_name} CODE offset {}: {message}",
                    instruction.offset
                ),
            ));
        }
    }
    diagnostics
}

fn check(instruction: &BinaryInstruction, target: Option<BinaryTarget>) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("fragment CODE requires target(gpu)");
    }
    if instruction.modifiers != 0 {
        return Some("fragment CODE forbids instruction modifier bits");
    }
    let expected_results = u64::from(matches!(instruction.opcode, 0x406 | 0x409));
    if instruction.result_count != expected_results {
        return Some("fragment CODE has wrong SSA result count");
    }
    match instruction.opcode {
        0x406 => match instruction.operands.as_slice() {
            [BinaryOperand::Integer(index)]
                if !index.negative && index.magnitude <= u128::from(u32::MAX) =>
            {
                None
            }
            _ => Some("frag.load_attachment requires an unsigned u32 index"),
        },
        0x405 | 0x407 | 0x408 | 0x409 => {
            if instruction.operands.is_empty() {
                None
            } else {
                Some("fragment CODE has unexpected operands")
            }
        }
        _ => Some("unknown fragment opcode"),
    }
}
