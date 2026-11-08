//! V2 checks for scalar texture atomic CODE (0x48A).
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

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
        if instruction.opcode != 0x48a {
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
        return Some("tex.atomic CODE requires target(gpu)");
    }
    if instruction.result_count != 1 {
        return Some("tex.atomic CODE requires exactly one SSA result");
    }
    if instruction.modifiers != 0 {
        return Some("tex.atomic CODE forbids sparse, selector and unchecked_uniformity modifiers");
    }
    let [
        BinaryOperand::Values(inputs),
        BinaryOperand::Integers(offsets),
        BinaryOperand::Index(operation),
    ] = instruction.operands.as_slice()
    else {
        return Some("tex.atomic CODE needs SSA values, offsets and operation ID");
    };
    if !matches!(inputs.len(), 3 | 4) {
        return Some("tex.atomic CODE has invalid SSA operand count");
    }
    if !offsets.is_empty() {
        return Some("tex.atomic CODE does not permit constant offsets");
    }
    if *operation > 14 {
        return Some("tex.atomic CODE operation ID must be within 0–14");
    }
    None
}

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @AtomicV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @AtomicV2.int(%tex: texture<2darray, i32, readwrite>, %coord: vec<2 x i32>, %layer: i32, %value: i32) -> i32 target(gpu) {\nbb0(%tex: texture<2darray, i32, readwrite>, %coord: vec<2 x i32>, %layer: i32, %value: i32):\n  %result = tex.atomic %tex, %coord, %layer, add %value\n  return %result\n}\n\nfunc module @AtomicV2.float(%tex: texture<2d, f32, readwrite>, %coord: vec<2 x i32>, %value: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, readwrite>, %coord: vec<2 x i32>, %value: f32):\n  %result = tex.atomic %tex, %coord, fadd %value\n  return %result\n}\n";

    fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
        let parsed = parse_text_module(SOURCE).expect("texture atomic source parses");
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("texture atomic CODE assembles"),
        )
        .expect("TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        (binary, signatures)
    }

    #[test]
    fn v2_atomic_text_binary_and_byte_identical_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("atomic source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "valid atomic text rejected: {errors:?}");
        let (binary, _) = decoded();
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "valid atomic binary rejected: {errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("CODE assembles");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical atomic text parses")
            )
            .expect("CODE reassembles")
        );
    }

    #[test]
    fn v2_atomic_binary_rejects_cpu_result_and_modifier_forgery() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decodes");
        let body = code.functions.first_mut().expect("atomic function");
        let errors = verify(body, Some(BinaryTarget::Cpu), "int");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU atomic texture escaped V2: {errors:?}"
        );
        let op = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48a)
            .expect("atomic opcode");
        op.result_count = 0;
        let errors = verify(body, Some(BinaryTarget::Gpu), "int");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "missing atomic result escaped V2: {errors:?}"
        );
        let op = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48a)
            .expect("atomic opcode");
        op.result_count = 1;
        op.modifiers = 1 << 35;
        let errors = verify(body, Some(BinaryTarget::Gpu), "int");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "sparse atomic texture escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_atomic_binary_rejects_bad_operation_offset_and_operand_shape() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decodes");
        let body = code.functions.get_mut(1).expect("float atomic function");
        let op = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48a)
            .expect("atomic opcode");
        op.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2]),
            BinaryOperand::Integers(Vec::new()),
            BinaryOperand::Index(15),
        ];
        let errors = verify(body, Some(BinaryTarget::Gpu), "float");
        assert!(
            errors.iter().any(|issue| issue.message.contains("0–14")),
            "out-of-range atomic operation escaped V2: {errors:?}"
        );
        let op = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48a)
            .expect("atomic opcode");
        op.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2]),
            BinaryOperand::Integers(vec![tir_format::leb::IntegerImmediate {
                negative: false,
                magnitude: 1,
            }]),
            BinaryOperand::Index(11),
        ];
        let errors = verify(body, Some(BinaryTarget::Gpu), "float");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("constant offsets")),
            "atomic texture with offset escaped V2: {errors:?}"
        );
        let op = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48a)
            .expect("atomic opcode");
        op.operands = vec![
            BinaryOperand::Values(vec![0, 1]),
            BinaryOperand::Integers(Vec::new()),
            BinaryOperand::Index(11),
        ];
        let errors = verify(body, Some(BinaryTarget::Gpu), "float");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("SSA operand count")),
            "atomic texture with missing value escaped V2: {errors:?}"
        );
    }
}
