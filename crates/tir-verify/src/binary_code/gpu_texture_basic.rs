//! Binary V2 structural checks for texture query, read and write CODE.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const SELECTOR_MASK: u64 = 0xf << 48;
const SPARSE: u64 = 1 << 35;

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
        if !matches!(instruction.opcode, 0x488 | 0x489 | 0x48b) {
            continue;
        }
        if let Some(error) = check(instruction, target) {
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

fn check(instruction: &BinaryInstruction, target: Option<BinaryTarget>) -> Option<&'static str> {
    if target != Some(BinaryTarget::Gpu) {
        return Some("texture CODE requires target(gpu)");
    }
    let selector = (instruction.modifiers & SELECTOR_MASK) >> 48;
    let allowed = SELECTOR_MASK
        | if instruction.opcode == 0x488 {
            SPARSE
        } else {
            0
        };
    if instruction.modifiers & !allowed != 0
        || (instruction.opcode == 0x48b && selector > 6)
        || (instruction.opcode != 0x48b && selector != 0)
    {
        return Some("texture query/read/write modifier or selector is invalid");
    }
    let results = u64::from(instruction.opcode != 0x489)
        + u64::from(instruction.opcode == 0x488 && instruction.modifiers & SPARSE != 0);
    if instruction.result_count != results {
        return Some("texture query/read/write SSA result count is invalid");
    }
    let [
        BinaryOperand::Values(values),
        BinaryOperand::Integers(offsets),
    ] = instruction.operands.as_slice()
    else {
        return Some("texture CODE needs a counted SSA list and integer offset list");
    };
    if !offsets.is_empty() {
        return Some("texture query/read/write does not support nonempty offsets");
    }
    if values.len() < if instruction.opcode == 0x48b { 1 } else { 2 } {
        return Some("texture CODE has too few SSA value operands");
    }
    if instruction.opcode == 0x48b && values.len() != 1 {
        return Some("tex.query requires exactly one texture handle");
    }
    None
}

#[cfg(test)]
#[path = "gpu_texture_sparse_tests.rs"]
mod sparse_conformance;

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @TextureV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @TextureV2.query(%tex: texture<2d, i32, read>) -> vec<2 x i32> target(gpu) {\nbb0(%tex: texture<2d, i32, read>):\n  %size = tex.query %tex, size\n  return %size\n}\n\nfunc module @TextureV2.read(%tex: texture<2d, i32, read>, %coords: vec<2 x i32>) -> i32 target(gpu) {\nbb0(%tex: texture<2d, i32, read>, %coords: vec<2 x i32>):\n  %value = tex.read %tex, %coords\n  return %value\n}\n\nfunc module @TextureV2.write(%tex: texture<2d, i32, write>, %coords: vec<2 x i32>, %data: i32) -> unit target(gpu) {\nbb0(%tex: texture<2d, i32, write>, %coords: vec<2 x i32>, %data: i32):\n  tex.write %tex, %coords, %data\n  return\n}\n";

    fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
        let parsed = parse_text_module(SOURCE).expect("texture V2 fixture parses");
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("texture fixture assembles"),
        )
        .expect("fixture TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        (binary, signatures)
    }

    #[test]
    fn v2_positive_text_binary_and_canonical_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("texture V2 text parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "valid texture V2 text rejected: {errors:?}"
        );
        let (binary, _) = decoded();
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "valid texture binary V2 rejected: {errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("texture CODE assembles");
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("texture CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("texture canonical text parses")
            )
            .expect("texture CODE reassembles")
        );
    }

    #[test]
    fn v2_rejects_cpu_target_wrong_results_and_query_selector() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let query = code.functions.get_mut(0).expect("query function");
        let errors = verify(query, Some(BinaryTarget::Cpu), "query");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU texture CODE escaped V2: {errors:?}"
        );
        let first = query.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x48b)
            .expect("tex.query");
        first.result_count = 0;
        let errors = verify(query, Some(BinaryTarget::Gpu), "query");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("result count")),
            "missing query SSA result escaped V2: {errors:?}"
        );
        let first = query.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x48b)
            .expect("tex.query");
        first.result_count = 1;
        first.modifiers = 7 << 48;
        let errors = verify(query, Some(BinaryTarget::Gpu), "query");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("selector")),
            "unknown texture query selector escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_rejects_forged_texture_offsets_and_argument_shapes() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let read = code.functions.get_mut(1).expect("read function");
        let first = read.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x488)
            .expect("tex.read");
        first.operands = vec![
            BinaryOperand::Values(vec![0]),
            BinaryOperand::Integers(Vec::new()),
        ];
        let errors = verify(read, Some(BinaryTarget::Gpu), "read");
        assert!(
            errors.iter().any(|issue| issue.message.contains("too few")),
            "read with too few operands escaped V2: {errors:?}"
        );
        let first = read.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x488)
            .expect("tex.read");
        first.modifiers = 1 << 35;
        let errors = verify(read, Some(BinaryTarget::Gpu), "read");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("result count")),
            "sparse read without residency SSA result escaped V2: {errors:?}"
        );
        let first = read.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x488)
            .expect("tex.read");
        first.result_count = 2;
        first.operands = vec![
            BinaryOperand::Values(vec![0, 1]),
            BinaryOperand::Integers(Vec::new()),
        ];
        let errors = verify(read, Some(BinaryTarget::Gpu), "read");
        assert!(
            errors.is_empty(),
            "binary V2 rejected a normatively valid sparse read: {errors:?}"
        );
        let first = read.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE")
            .iter_mut()
            .find(|op| op.opcode == 0x488)
            .expect("tex.read");
        first.modifiers = 0;
        first.result_count = 1;
        first.operands = vec![
            BinaryOperand::Values(vec![0, 1]),
            BinaryOperand::Integers(vec![tir_format::leb::IntegerImmediate {
                negative: false,
                magnitude: 1,
            }]),
        ];
        let errors = verify(read, Some(BinaryTarget::Gpu), "read");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("nonempty offsets")),
            "nonempty texture offset escaped V2: {errors:?}"
        );
    }
}
