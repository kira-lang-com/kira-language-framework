//! V2 checks for object/mesh CODE (0x4c0–0x4c4), result counts and bit-34 mods.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

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
        if !matches!(operation.opcode, 0x4c0..=0x4c4) {
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
        return Some("mesh/object CODE requires target(gpu)");
    }
    if instruction.result_count != 0 {
        return Some("mesh/object CODE must define zero SSA results");
    }
    let allowed = if matches!(instruction.opcode, 0x4c0 | 0x4c1) {
        UNCHECKED_UNIFORMITY
    } else {
        0
    };
    if instruction.modifiers & !allowed != 0 {
        return Some("mesh/object modifier bits are invalid");
    }
    let expected = if instruction.opcode == 0x4c0 { 3 } else { 2 };
    match instruction.operands.as_slice() {
        [BinaryOperand::Values(values)] if values.len() == expected => None,
        _ => Some("mesh/object CODE SSA operand count or shape is invalid"),
    }
}

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @MeshCheck tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @MeshCheck.dispatch() -> unit target(gpu) {\nbb0:\n  %x = gpu.spec_const 1, i32, 4\n  %y = gpu.spec_const 2, i32, 5\n  %z = gpu.spec_const 3, i32, 6\n  object.dispatch_mesh %x, %y, %z [unchecked_uniformity]\n  return\n}\n\nfunc module @MeshCheck.counts() -> unit target(gpu) {\nbb0:\n  %vertices = gpu.spec_const 4, i32, 8\n  %primitives = gpu.spec_const 5, i32, 4\n  mesh.set_counts %vertices, %primitives\n  mesh.write.vertex %primitives, %vertices\n  mesh.write.primitive %primitives, %vertices\n  mesh.write.indices %primitives, %vertices\n  return\n}\n";

    fn decoded() -> (
        BinaryModule,
        tir_format::BinaryTypeTable,
        tir_format::BinarySignatureTable,
    ) {
        let source = parse_text_module(SOURCE).expect("mesh source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&source).expect("mesh CODE assembles"))
                .expect("mesh TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("mesh TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("mesh SIGNATURES decode");
        (binary, types, signatures)
    }

    #[test]
    fn mesh_code_v2_text_and_binary_verify_with_byte_identical_roundtrip() {
        let parsed = parse_text_module(SOURCE).expect("mesh source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "GPU mesh text V2 rejected: {errors:?}");
        let (binary, _, _) = decoded();
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "GPU mesh binary V2 rejected: {errors:?}");
        let bytes = assemble_text_module(&parsed).expect("mesh CODE encodes");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("mesh CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("mesh canonical source parses")
            )
            .expect("mesh reassembles")
        );
    }

    #[test]
    fn mesh_v2_rejects_cpu_target_modifier_and_result_forgery() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("mesh CODE decode");
        let body = code.functions.first_mut().expect("dispatch function");
        let issues = verify(body, Some(BinaryTarget::Cpu), "dispatch");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU mesh escaped V2: {issues:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("mesh CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x4c0)
            .expect("object.dispatch_mesh");
        first.result_count = 1;
        let issues = verify(body, Some(BinaryTarget::Gpu), "dispatch");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("zero SSA results")),
            "forged mesh result escaped V2: {issues:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("mesh CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x4c0)
            .expect("object.dispatch_mesh");
        first.result_count = 0;
        first.modifiers = 1 << 48;
        let issues = verify(body, Some(BinaryTarget::Gpu), "dispatch");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("modifier")),
            "invalid mesh modifier escaped V2: {issues:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("mesh CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x4c0)
            .expect("object.dispatch_mesh");
        first.modifiers = 0;
        first.operands = vec![BinaryOperand::Values(vec![0, 1])];
        let issues = verify(body, Some(BinaryTarget::Gpu), "dispatch");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("operand count")),
            "wrong dispatch operand count escaped V2: {issues:?}"
        );
    }

    #[test]
    fn mesh_v2_rejects_write_modifiers_and_missing_operands() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("mesh CODE decode");
        let body = code.functions.get_mut(1).expect("mesh function");
        let write = body.blocks[0]
            .instructions
            .as_mut()
            .expect("mesh CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x4c2)
            .expect("mesh.write.vertex");
        write.modifiers = UNCHECKED_UNIFORMITY;
        let issues = verify(body, Some(BinaryTarget::Gpu), "counts");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("modifier")),
            "mesh output write modifier escaped V2: {issues:?}"
        );
        let write = body.blocks[0]
            .instructions
            .as_mut()
            .expect("mesh CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x4c2)
            .expect("mesh.write.vertex");
        write.modifiers = 0;
        write.operands = vec![BinaryOperand::Value(0)];
        let issues = verify(body, Some(BinaryTarget::Gpu), "counts");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("operand count")),
            "mesh output with missing operands escaped V2: {issues:?}"
        );
    }
}
