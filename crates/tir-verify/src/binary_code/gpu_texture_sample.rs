//! Binary V2 checks for typed texture sampling and feedback (0x480–0x487, 0x48c–0x48d).
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
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
        if !matches!(instruction.opcode, 0x480..=0x487 | 0x48c | 0x48d) {
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
        return Some("tex.sample/tex.lod requires target(gpu)");
    }
    let sparse = instruction.modifiers & SPARSE != 0;
    if instruction.result_count != 1 + u64::from(sparse) {
        return Some("texture sampling requires one SSA result, plus residency for sparse");
    }
    let allowed = if matches!(instruction.opcode, 0x481 | 0x482 | 0x485) {
        0
    } else if instruction.opcode == 0x486 {
        UNCHECKED_UNIFORMITY | (0xf << 48)
    } else {
        UNCHECKED_UNIFORMITY
    };
    let allowed = allowed
        | if instruction.opcode == 0x48c {
            0
        } else {
            SPARSE
        };
    if instruction.modifiers & !allowed != 0 {
        return Some("texture sampling modifiers must contain only unchecked_uniformity");
    }
    if instruction.opcode == 0x486 && (instruction.modifiers >> 48) & 0xf > 3 {
        return Some("gather component selector must be within 0–3");
    }
    let [
        BinaryOperand::Values(values),
        BinaryOperand::Integers(offsets),
    ] = instruction.operands.as_slice()
    else {
        return Some("texture sampling requires counted SSA values and integer offsets");
    };
    let valid_count = match instruction.opcode {
        0x480 => (3..=5).contains(&values.len()),
        0x481 | 0x483 => (4..=6).contains(&values.len()),
        0x482 => (5..=7).contains(&values.len()),
        0x484 => (4..=6).contains(&values.len()),
        0x485 => (5..=7).contains(&values.len()),
        0x486 => (3..=5).contains(&values.len()),
        0x487 => (4..=6).contains(&values.len()),
        0x48d => (3..=5).contains(&values.len()),
        _ => (3..=4).contains(&values.len()),
    };
    if !valid_count {
        return Some("texture sample/LOD SSA operand count is invalid");
    }
    if instruction.opcode == 0x48c && !offsets.is_empty() {
        return Some("tex.lod cannot carry constant offsets");
    }
    if offsets.len() > 3
        || offsets.iter().any(|offset| {
            (offset.negative && (offset.magnitude == 0 || offset.magnitude > 8))
                || (!offset.negative && offset.magnitude > 7)
        })
    {
        return Some("texture sample offset outside -8..=7 or has malformed sign");
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

    const SOURCE: &str = "module @SamplingV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @SamplingV2.sample(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %min: f32):\n  %result = tex.sample %tex, %smp, %uv offset [-8, 7] min_lod %min [unchecked_uniformity]\n  return %result\n}\n\nfunc module @SamplingV2.lod(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>) -> vec<2 x f32> target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>):\n  %result = tex.lod %tex, %smp, %uv\n  return %result\n}\n";

    fn decoded() -> (BinaryModule, tir_format::BinarySignatureTable) {
        let parsed = parse_text_module(SOURCE).expect("sampling V2 source parses");
        let bytes = assemble_text_module(&parsed).expect("sampling V2 fixture assembles");
        let binary = BinaryModule::decode(&bytes).expect("sampling V2 binary decodes");
        let types = binary.type_table(u64::MAX).expect("sampling TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("sampling SIGNATURES decode");
        (binary, signatures)
    }

    #[test]
    fn v2_text_and_binary_verify_sample_and_lod_and_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("sampling V2 source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "sampling V2/V15 text rejected: {errors:?}"
        );
        let (binary, _) = decoded();
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "sampling binary V2 rejected: {errors:?}");
        let bytes = assemble_text_module(&parsed).expect("sampling CODE assembles");
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("sampling CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical sampling parses")
            )
            .expect("sampling reassembles")
        );
    }

    #[test]
    fn v2_binary_rejects_cpu_result_count_sparse_and_selector_bits() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("sampling CODE decode");
        let body = code.functions.get_mut(0).expect("sample function");
        let issues = verify(body, Some(BinaryTarget::Cpu), "sample");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU sampling escaped V2: {issues:?}"
        );
        let sample = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x480)
            .expect("tex.sample");
        sample.result_count = 0;
        let issues = verify(body, Some(BinaryTarget::Gpu), "sample");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "sample missing result escaped V2: {issues:?}"
        );
        let sample = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x480)
            .expect("tex.sample");
        sample.result_count = 1;
        sample.modifiers = 1 << 35;
        let issues = verify(body, Some(BinaryTarget::Gpu), "sample");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("residency")),
            "sparse sample without residency result escaped V2: {issues:?}"
        );
        let sample = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x480)
            .expect("tex.sample");
        sample.modifiers = 1 << 48;
        let issues = verify(body, Some(BinaryTarget::Gpu), "sample");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "sample selector bits escaped V2: {issues:?}"
        );
    }

    #[test]
    fn v2_binary_rejects_wrong_arity_offsets_and_negative_zero() {
        let (binary, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("sampling CODE decode");
        let body = code.functions.get_mut(1).expect("LOD function");
        let lod = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48c)
            .expect("tex.lod");
        lod.operands = vec![
            BinaryOperand::Values(vec![0, 1]),
            BinaryOperand::Integers(Vec::new()),
        ];
        let issues = verify(body, Some(BinaryTarget::Gpu), "lod");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("operand count")),
            "LOD with missing SSA operand escaped V2: {issues:?}"
        );
        let lod = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x48c)
            .expect("tex.lod");
        lod.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2]),
            BinaryOperand::Integers(vec![tir_format::leb::IntegerImmediate {
                negative: false,
                magnitude: 1,
            }]),
        ];
        let issues = verify(body, Some(BinaryTarget::Gpu), "lod");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("cannot carry constant offsets")),
            "LOD with offset escaped V2: {issues:?}"
        );
        let body = code.functions.get_mut(0).expect("sample function");
        let sample = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x480)
            .expect("tex.sample");
        sample.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2]),
            BinaryOperand::Integers(vec![tir_format::leb::IntegerImmediate {
                negative: true,
                magnitude: 0,
            }]),
        ];
        let issues = verify(body, Some(BinaryTarget::Gpu), "sample");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("malformed sign")),
            "negative-zero texture offset escaped V2: {issues:?}"
        );
    }
}

#[cfg(test)]
mod explicit_level_bias_conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @ExplicitSampling tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @ExplicitSampling.level(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %lod: f32):\n  %result = tex.sample_level %tex, %smp, %uv, lod %lod offset [-8, 7]\n  return %result\n}\n\nfunc module @ExplicitSampling.bias(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %bias: f32, %min: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %bias: f32, %min: f32):\n  %result = tex.sample_bias %tex, %smp, %uv, bias %bias offset [2, -3] min_lod %min [unchecked_uniformity]\n  return %result\n}\n";

    #[test]
    fn explicit_level_and_bias_pass_v2_text_binary_and_canonical_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("explicit sampling text parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "valid explicit sampling text rejected: {errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("explicit sampling CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("explicit sampling TIRB decodes");
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "valid explicit sampling binary rejected: {errors:?}"
        );
        let canonical = disassemble_text_module(&binary, u64::MAX)
            .expect("explicit sampling CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical explicit sampling parses")
            )
            .expect("explicit sampling reassembles")
        );
    }

    #[test]
    fn explicit_level_and_bias_reject_forged_binary_modifiers_and_result_counts() {
        let parsed = parse_text_module(SOURCE).expect("explicit sampling source parses");
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("explicit sampling CODE assembles"),
        )
        .expect("explicit sampling TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let level = code.functions.get_mut(0).expect("level function");
        let operation = level.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x481)
            .expect("tex.sample_level");
        operation.modifiers = UNCHECKED_UNIFORMITY;
        let errors = verify(level, Some(BinaryTarget::Gpu), "level");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "explicit LOD accepted unchecked-uniformity bit: {errors:?}"
        );
        let bias = code.functions.get_mut(1).expect("bias function");
        let operation = bias.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x483)
            .expect("tex.sample_bias");
        operation.result_count = 2;
        let errors = verify(bias, Some(BinaryTarget::Gpu), "bias");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "bias sampling with two SSA results escaped V2: {errors:?}"
        );
    }
}

#[cfg(test)]
mod explicit_grad_conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @GradientSampling tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @GradientSampling.grad(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %min: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %dx: vec<2 x f32>, %dy: vec<2 x f32>, %min: f32):\n  %result = tex.sample_grad %tex, %smp, %uv grad %dx, %dy offset [-8, 7] min_lod %min\n  return %result\n}\n";

    #[test]
    fn gradient_sampling_text_and_binary_v2_pass_and_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("gradient source parses");
        let issues = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            issues.is_empty(),
            "gradient sample text V2/V15 rejected: {issues:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("gradient CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("gradient TIRB decodes");
        let issues = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            issues.is_empty(),
            "gradient sample binary V2 rejected: {issues:?}"
        );
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("gradient CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("canonical gradient CODE parses")
            )
            .expect("gradient CODE reassembles")
        );
    }

    #[test]
    fn gradient_sampling_binary_rejects_wrong_results_and_modifiers() {
        let parsed = parse_text_module(SOURCE).expect("gradient source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&parsed).expect("gradient CODE assembles"))
                .expect("gradient TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.first_mut().expect("gradient function");
        let operation = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x482)
            .expect("tex.sample_grad");
        operation.modifiers = UNCHECKED_UNIFORMITY;
        let errors = verify(body, Some(BinaryTarget::Gpu), "grad");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "grad accepted unchecked_uniformity: {errors:?}"
        );
        let operation = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x482)
            .expect("tex.sample_grad");
        operation.modifiers = 0;
        operation.result_count = 2;
        let errors = verify(body, Some(BinaryTarget::Gpu), "grad");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "grad with two results escaped V2: {errors:?}"
        );
        let operation = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x482)
            .expect("tex.sample_grad");
        operation.result_count = 1;
        operation.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2, 3]),
            BinaryOperand::Integers(Vec::new()),
        ];
        let errors = verify(body, Some(BinaryTarget::Gpu), "grad");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("SSA operand count")),
            "grad missing second gradient escaped V2: {errors:?}"
        );
    }
}

#[cfg(test)]
mod explicit_compare_conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @ComparisonV2 tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @ComparisonV2.compare(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %reference: f32, %min: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %reference: f32, %min: f32):\n  %result = tex.sample_compare %tex, %smp, %uv compare %reference offset [-8, 7] min_lod %min [unchecked_uniformity]\n  return %result\n}\n\nfunc module @ComparisonV2.explicit(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %reference: f32, %lod: f32) -> f32 target(gpu) {\nbb0(%tex: texture<2d, f32, sample>, %smp: sampler, %uv: vec<2 x f32>, %reference: f32, %lod: f32):\n  %result = tex.sample_compare_level %tex, %smp, %uv compare %reference, lod %lod offset [1, -2]\n  return %result\n}\n";

    #[test]
    fn v2_comparison_text_binary_and_canonical_round_trip() {
        let parsed = parse_text_module(SOURCE).expect("comparison sampling source parses");
        let issues = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            issues.is_empty(),
            "valid comparison sampling text V2 rejected: {issues:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("comparison sampling CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("comparison sampling TIRB decodes");
        let issues = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            issues.is_empty(),
            "valid comparison sampling binary rejected: {issues:?}"
        );
        let canonical =
            disassemble_text_module(&binary, u64::MAX).expect("comparison CODE disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("comparison canonical text parses")
            )
            .expect("comparison CODE reassembles")
        );
    }

    #[test]
    fn v2_comparison_binary_rejects_forged_results_selectors_and_arity() {
        let parsed = parse_text_module(SOURCE).expect("comparison sampling source parses");
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("comparison sampling CODE assembles"),
        )
        .expect("comparison TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.get_mut(0).expect("comparison function");
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x484)
            .expect("tex.sample_compare");
        first.result_count = 2;
        let issues = verify(body, Some(BinaryTarget::Gpu), "compare");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("one SSA result")),
            "comparison with two results escaped V2: {issues:?}"
        );
        let body = code
            .functions
            .get_mut(1)
            .expect("explicit comparison function");
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x485)
            .expect("tex.sample_compare_level");
        first.modifiers = UNCHECKED_UNIFORMITY;
        let issues = verify(body, Some(BinaryTarget::Gpu), "explicit");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "explicit comparison with unchecked uniformity escaped V2: {issues:?}"
        );
        let first = body.blocks[0]
            .instructions
            .as_mut()
            .expect("CODE instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x485)
            .expect("tex.sample_compare_level");
        first.modifiers = 0;
        first.operands = vec![
            BinaryOperand::Values(vec![0, 1, 2]),
            BinaryOperand::Integers(Vec::new()),
        ];
        let issues = verify(body, Some(BinaryTarget::Gpu), "explicit");
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("operand count")),
            "explicit comparison missing reference/LOD escaped V2: {issues:?}"
        );
    }
}

#[cfg(test)]
#[path = "gpu_texture_gather_tests.rs"]
mod gather_conformance;

#[cfg(test)]
#[path = "gpu_texture_feedback_tests.rs"]
mod feedback_conformance;

#[cfg(test)]
#[path = "gpu_texture_sample_sparse_tests.rs"]
mod sparse_conformance;
