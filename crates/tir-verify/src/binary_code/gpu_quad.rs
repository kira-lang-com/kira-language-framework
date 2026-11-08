//! Binary V2 structural and GPU-target checks for derivatives and quad collectives.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const FINE: u64 = 1 << 36;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
const SELECTOR_MASK: u64 = 0xf << 48;

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
        if !matches!(instruction.opcode, 0x420..=0x426) {
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
        return Some("GPU derivative/quad CODE requires target(gpu)");
    }
    if instruction.result_count != 1 {
        return Some("GPU derivative/quad CODE must define one SSA result");
    }
    let allowed = UNCHECKED_UNIFORMITY
        | match instruction.opcode {
            0x420 | 0x421 => FINE,
            0x424 => SELECTOR_MASK,
            _ => 0,
        };
    if instruction.modifiers & !allowed != 0
        || (instruction.opcode == 0x424 && (instruction.modifiers & SELECTOR_MASK) >> 48 > 2)
    {
        return Some("GPU derivative/quad modifier or selector is invalid");
    }
    let valid = match instruction.opcode {
        0x423 => matches!(instruction.operands.as_slice(),
            [BinaryOperand::Values(arguments)] if arguments.len() == 2),
        _ => matches!(instruction.operands.as_slice(), [BinaryOperand::Value(_)]),
    };
    (!valid).then_some("GPU derivative/quad CODE SSA operand shape is invalid")
}

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @QuadCheck tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @QuadCheck.diff() -> f32 target(gpu) {\nbb0:\n  %value = gpu.spec_const 1, f32, 1.5\n  %dx = gpu.ddx %value [fine]\n  %dy = gpu.ddy %dx\n  %width = gpu.fwidth %dy\n  return %width\n}\n\nfunc module @QuadCheck.broadcast() -> i32 target(gpu) {\nbb0:\n  %data = gpu.spec_const 2, i32, 9\n  %lane = gpu.spec_const 3, i32, 3\n  %broadcast = quad.broadcast %data, %lane\n  %swapped = quad.swap %broadcast [diagonal]\n  return %swapped\n}\n\nfunc module @QuadCheck.predicate() -> i1 target(gpu) {\nbb0:\n  %predicate = gpu.spec_const 4, i1, true\n  %all = quad.all %predicate\n  %any = quad.any %all\n  return %any\n}\n";

    #[test]
    fn v2_text_and_binary_verify_all_seven_quad_opcodes_and_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("GPU quad source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "GPU quad text V2/V15 failed: {errors:?}");
        let bytes = assemble_text_module(&parsed).expect("GPU quad CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("GPU quad TIRB decodes");
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "GPU quad binary V2 failed: {errors:?}");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU quad disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical GPU quad text parses")
            )
            .expect("GPU quad reassembles")
        );
    }

    #[test]
    fn v2_binary_rejects_cpu_quad_wrong_result_modifier_and_operand_shape() {
        let parsed = parse_text_module(SOURCE).expect("GPU quad source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&parsed).expect("GPU quad CODE encodes"))
                .expect("GPU quad TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decodes");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decodes");
        let body = code.functions.first_mut().expect("one function");
        let errors = verify(body, Some(BinaryTarget::Cpu), "diff");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU-targeted derivatives escaped V2: {errors:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x420)
            .expect("gpu.ddx");
        first.result_count = 0;
        let errors = verify(body, Some(BinaryTarget::Gpu), "diff");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "derivative with no result escaped V2: {errors:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x420)
            .expect("gpu.ddx");
        first.result_count = 1;
        first.modifiers = 1 << 48;
        let errors = verify(body, Some(BinaryTarget::Gpu), "diff");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("modifier")),
            "derivative selector escaped V2: {errors:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x420)
            .expect("gpu.ddx");
        first.modifiers = 0;
        first.operands = vec![BinaryOperand::Values(vec![0, 0])];
        let errors = verify(body, Some(BinaryTarget::Gpu), "diff");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("SSA operand shape")),
            "derivative with two operands escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_binary_rejects_invalid_quad_swap_selector_and_broadcast_arity() {
        let parsed = parse_text_module(SOURCE).expect("GPU quad source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&parsed).expect("GPU quad CODE encodes"))
                .expect("GPU quad TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decodes");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decodes");
        let body = code.functions.get_mut(1).expect("broadcast function");
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x424)
            .expect("quad.swap");
        first.modifiers = 3 << 48;
        let errors = verify(body, Some(BinaryTarget::Gpu), "broadcast");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("selector")),
            "invalid quad swap selector escaped V2: {errors:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x424)
            .expect("quad.swap");
        first.modifiers = 0;
        let broadcast = body.blocks[0]
            .instructions
            .as_mut()
            .expect("decoded CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x423)
            .expect("quad.broadcast");
        broadcast.operands = vec![BinaryOperand::Value(0)];
        let errors = verify(body, Some(BinaryTarget::Gpu), "broadcast");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("SSA operand shape")),
            "broadcast with one operand escaped V2: {errors:?}"
        );
    }
}
