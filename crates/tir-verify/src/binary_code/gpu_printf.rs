use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget, StringTable};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    strings: Option<&StringTable>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if instruction.opcode != 0x40a {
            continue;
        }
        if let Some(message) = check(instruction, target, strings) {
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

fn check(
    instruction: &BinaryInstruction,
    target: Option<BinaryTarget>,
    strings: Option<&StringTable>,
) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("gpu.printf requires target(gpu)");
    }
    if instruction.modifiers != 0 || instruction.result_count != 0 {
        return Some("gpu.printf requires zero modifiers and zero SSA results");
    }
    let [
        BinaryOperand::Index(format),
        BinaryOperand::Values(arguments),
    ] = instruction.operands.as_slice()
    else {
        return Some("gpu.printf requires a STRINGS index and counted SSA argument list");
    };
    if !strings.is_some_and(|strings| {
        usize::try_from(*format)
            .ok()
            .and_then(|index| strings.get(index))
            .is_some()
    }) {
        return Some("gpu.printf format STRINGS index is missing or invalid");
    }
    if arguments.len() >= (1usize << 32) {
        return Some("gpu.printf argument count exceeds the 32-bit CODE limit");
    }
    None
}
