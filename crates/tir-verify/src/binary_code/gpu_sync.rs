//! V2 checks for the 22.7 five-domain GPU synchronization CODE mask.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const SCOPE_MASK: u64 = 7 << 26;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut issues = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if !matches!(instruction.opcode, 0x402 | 0x403) {
            continue;
        }
        if let Some(message) = check(instruction, target) {
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

fn check(instruction: &BinaryInstruction, target: Option<BinaryTarget>) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("GPU synchronization requires target(gpu)");
    }
    if instruction.result_count != 0 {
        return Some("GPU synchronization must have zero SSA results");
    }
    let barrier = instruction.opcode == 0x402;
    let allowed = SCOPE_MASK | if barrier { UNCHECKED_UNIFORMITY } else { 0 };
    let scope = (instruction.modifiers & SCOPE_MASK) >> 26;
    if instruction.modifiers & !allowed != 0
        || (barrier && !matches!(scope, 0 | 2 | 3))
        || (!barrier && scope > 5)
    {
        return Some("GPU synchronization scope or modifier bits are invalid");
    }
    let [BinaryOperand::Index(mask)] = instruction.operands.as_slice() else {
        return Some("GPU synchronization requires one memory-domain byte");
    };
    if *mask == 0 || *mask > 0x1f {
        return Some("GPU synchronization memory-domain mask must be nonzero and five bits");
    }
    None
}
