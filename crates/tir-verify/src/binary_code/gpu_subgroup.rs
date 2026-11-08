//! Binary V2 subgroup CODE structural checks and typed reduction fields.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(crate) fn verify(
    body: &BinaryFunctionBody,
    target: Option<BinaryTarget>,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for operation in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if !matches!(operation.opcode, 0x440..=0x452) {
            continue;
        }
        if let Some(message) = check(operation, target) {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "function @{function_name} CODE offset {}: {message}",
                    operation.offset
                ),
            ));
        }
    }
    diagnostics
}

fn check(instruction: &BinaryInstruction, target: Option<BinaryTarget>) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("subgroup CODE requires target(gpu)");
    }
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Some("subgroup CODE requires zero modifiers and exactly one SSA result");
    }
    let valid = match (instruction.opcode, instruction.operands.as_slice()) {
        (0x440, []) => true,
        (0x444 | 0x44b..=0x44f, [BinaryOperand::Values(values)]) => values.len() == 2,
        (
            0x450..=0x452,
            [
                BinaryOperand::Value(_),
                BinaryOperand::Index(operator),
                BinaryOperand::Index(cluster),
            ],
        ) => {
            if *operator > 14 {
                return Some("subgroup operator code must be 0..14");
            }
            if *cluster != 0 && (*cluster > 128 || !cluster.is_power_of_two()) {
                return Some("subgroup cluster size must be a power of two no greater than 128");
            }
            true
        }
        (0x441..=0x443 | 0x445..=0x44a, [BinaryOperand::Value(_)]) => true,
        _ => false,
    };
    (!valid).then_some("subgroup CODE operand count or operand shape is invalid")
}

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @SubgroupCheck tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @SubgroupCheck.vote() -> i1 target(gpu) {\nbb0:\n  %predicate = gpu.spec_const 1, i1, true\n  %elect = subgroup.elect\n  %all = subgroup.all %predicate\n  %any = subgroup.any %all\n  return %any\n}\n\nfunc module @SubgroupCheck.ballot() -> i32 target(gpu) {\nbb0:\n  %predicate = gpu.spec_const 2, i1, true\n  %mask = subgroup.ballot %predicate\n  %count = subgroup.ballot_bit_count %mask\n  return %count\n}\n\nfunc module @SubgroupCheck.scan() -> i32 target(gpu) {\nbb0:\n  %input = gpu.spec_const 3, i32, 7\n  %reduced = subgroup.reduce %input [add.s] [cluster 4]\n  %scanned = subgroup.scan_inclusive %reduced [xor]\n  return %scanned\n}\n";

    fn decoded() -> (
        BinaryModule,
        tir_format::BinaryTypeTable,
        tir_format::BinarySignatureTable,
    ) {
        let parsed = parse_text_module(SOURCE).expect("subgroup CODE source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&parsed).expect("subgroup CODE assembles"))
                .expect("subgroup TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        (binary, types, signatures)
    }

    #[test]
    fn v2_subgroup_text_binary_and_round_trip_verify() {
        let parsed = parse_text_module(SOURCE).expect("subgroup source parses");
        let text_errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            text_errors.is_empty(),
            "GPU subgroup V2/V15 text failed: {text_errors:?}"
        );
        let (binary, _, _) = decoded();
        let binary_errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            binary_errors.is_empty(),
            "GPU subgroup binary V2 failed: {binary_errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("subgroup CODE assembles");
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("subgroup CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical subgroup parses")
            )
            .expect("subgroup reassembles")
        );
    }

    #[test]
    fn v2_subgroup_cpu_result_modifier_and_arity_rejected() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.first_mut().expect("first function");
        let errors = verify(body, Some(BinaryTarget::Cpu), "vote");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU subgroup escaped V2: {errors:?}"
        );
        let elect = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x440)
            .expect("subgroup.elect");
        elect.result_count = 0;
        let errors = verify(body, Some(BinaryTarget::Gpu), "vote");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "subgroup without result escaped V2: {errors:?}"
        );
        let elect = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x440)
            .expect("subgroup.elect");
        elect.result_count = 1;
        elect.modifiers = 1 << 34;
        let errors = verify(body, Some(BinaryTarget::Gpu), "vote");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("zero modifiers")),
            "subgroup unchecked modifier escaped V2: {errors:?}"
        );
        let elect = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x440)
            .expect("subgroup.elect");
        elect.modifiers = 0;
        elect.operands = vec![BinaryOperand::Value(0)];
        let errors = verify(body, Some(BinaryTarget::Gpu), "vote");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("operand shape")),
            "elect with unexpected operands escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_subgroup_operator_and_cluster_fields_reject_malformed_code() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.get_mut(2).expect("scan function");
        let reduced = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x450)
            .expect("subgroup.reduce");
        reduced.operands[1] = BinaryOperand::Index(15);
        let errors = verify(body, Some(BinaryTarget::Gpu), "scan");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("operator code")),
            "oversized subgroup operator escaped V2: {errors:?}"
        );
        let reduced = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x450)
            .expect("subgroup.reduce");
        reduced.operands[1] = BinaryOperand::Index(0);
        reduced.operands[2] = BinaryOperand::Index(3);
        let errors = verify(body, Some(BinaryTarget::Gpu), "scan");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("cluster size")),
            "non-power-of-two cluster escaped V2: {errors:?}"
        );
        let reduced = body.blocks[0]
            .instructions
            .as_mut()
            .expect("instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x450)
            .expect("subgroup.reduce");
        reduced.operands[2] = BinaryOperand::Index(128);
        reduced.operands.remove(1);
        let errors = verify(body, Some(BinaryTarget::Gpu), "scan");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("operand shape")),
            "missing subgroup operator escaped V2: {errors:?}"
        );
    }
}
