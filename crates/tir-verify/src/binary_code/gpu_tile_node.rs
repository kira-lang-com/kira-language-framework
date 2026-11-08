//! V2 structural checks for tile imageblock and GPU work-graph CODE.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const GROUP: u64 = 1 << 48;

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
        if !matches!(operation.opcode, 0x5a0 | 0x5c0..=0x5c5) {
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
        return Some("tile/node CODE requires target(gpu)");
    }
    let expected_results = u64::from(instruction.opcode != 0x5c4);
    if instruction.result_count != expected_results {
        return Some("tile/node CODE has an invalid SSA result count");
    }
    let allowed = if instruction.opcode == 0x5c2 {
        GROUP
    } else {
        0
    };
    if instruction.modifiers & !allowed != 0 {
        return Some("tile/node CODE has invalid modifiers or allocation selector");
    }
    let valid = match (instruction.opcode, instruction.operands.as_slice()) {
        (0x5c1 | 0x5c5, []) => true,
        (0x5c2 | 0x5c3, [BinaryOperand::Values(values)]) => values.len() == 2,
        (0x5a0 | 0x5c0 | 0x5c4, [BinaryOperand::Value(_)]) => true,
        _ => false,
    };
    (!valid).then_some("tile/node CODE SSA operand count or shape is invalid")
}

#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{verify_binary_module_records, verify_text_module};
    use tir_diag::Severity;
    use tir_format::{
        BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module,
    };

    const SOURCE: &str = "module @TileNode tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @TileNode.tile() -> ptr<tile> target(gpu) {\nbb0:\n  %p = gpu.spec_const 1, vec<2 x i32>, [1, 2]\n  %addr = tile.imageblock.addr %p\n  return %addr\n}\n\nfunc module @TileNode.node() -> i1 target(gpu) {\nbb0:\n  %out = gpu.spec_const 2, i32, 0\n  %count = node.input.count\n  %input = node.input.addr %out\n  %alloc = node.output.allocate %out, %count [group]\n  %address = node.output.addr %alloc, %out\n  node.output.commit %alloc\n  %done = node.finished_cross_group\n  return %done\n}\n";

    fn decoded() -> (
        BinaryModule,
        tir_format::BinaryTypeTable,
        tir_format::BinarySignatureTable,
    ) {
        let parsed = parse_text_module(SOURCE).expect("tile/node source parses");
        let binary =
            BinaryModule::decode(&assemble_text_module(&parsed).expect("tile/node CODE assembles"))
                .expect("tile/node TIRB decodes");
        let types = binary.type_table(u64::MAX).expect("TYPES decode");
        let signatures = binary
            .signature_table(u64::MAX, &types)
            .expect("SIGNATURES decode");
        (binary, types, signatures)
    }

    #[test]
    fn v2_text_binary_verify_and_round_trip_tile_and_node() {
        let parsed = parse_text_module(SOURCE).expect("tile/node source parses");
        let errors = verify_text_module(&parsed, Default::default())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "tile/node text V2/V15 rejected: {errors:?}"
        );
        let (binary, _, _) = decoded();
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "tile/node binary V2 rejected: {errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("tile/node CODE assembles");
        let canonical = disassemble_text_module(&binary, u64::MAX).expect("tile/node disassembles");
        assert_eq!(
            bytes,
            assemble_text_module(
                &parse_text_module(&canonical).expect("tile/node canonical text parses")
            )
            .expect("tile/node reassembles")
        );
    }

    #[test]
    fn v2_binary_rejects_cpu_tile_and_forged_result_counts() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.first_mut().expect("tile function");
        let errors = verify(body, Some(BinaryTarget::Cpu), "tile");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("target(gpu)")),
            "CPU tile operation escaped V2: {errors:?}"
        );
        let tile = body.blocks[0]
            .instructions
            .as_mut()
            .expect("tile instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5a0)
            .expect("tile.imageblock.addr");
        tile.result_count = 0;
        let errors = verify(body, Some(BinaryTarget::Gpu), "tile");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("result count")),
            "missing tile result escaped V2: {errors:?}"
        );
        let tile = body.blocks[0]
            .instructions
            .as_mut()
            .expect("tile instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5a0)
            .expect("tile.imageblock.addr");
        tile.result_count = 1;
        tile.modifiers = GROUP;
        let errors = verify(body, Some(BinaryTarget::Gpu), "tile");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("modifiers")),
            "invalid tile modifier escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_binary_rejects_invalid_node_output_selector_and_operand_shapes() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.get_mut(1).expect("node function");
        let alloc = body.blocks[0]
            .instructions
            .as_mut()
            .expect("node instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5c2)
            .expect("node.output.allocate");
        alloc.modifiers = 2 << 48;
        let errors = verify(body, Some(BinaryTarget::Gpu), "node");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("selector")),
            "invalid node mode escaped V2: {errors:?}"
        );
        let alloc = body.blocks[0]
            .instructions
            .as_mut()
            .expect("node instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5c2)
            .expect("node.output.allocate");
        alloc.modifiers = 0;
        alloc.operands = vec![BinaryOperand::Value(0)];
        let errors = verify(body, Some(BinaryTarget::Gpu), "node");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("operand count")),
            "missing node output count escaped V2: {errors:?}"
        );
        let commit = body.blocks[0]
            .instructions
            .as_mut()
            .expect("node instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5c4)
            .expect("node.output.commit");
        commit.result_count = 1;
        let errors = verify(body, Some(BinaryTarget::Gpu), "node");
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("result count")),
            "node commit with SSA result escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v2_binary_rejects_extra_node_count_and_finished_operands() {
        let (binary, _, signatures) = decoded();
        let mut code = binary
            .code_table(u64::MAX, &signatures)
            .expect("CODE decode");
        let body = code.functions.get_mut(1).expect("node function");
        let count = body.blocks[0]
            .instructions
            .as_mut()
            .expect("node instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5c1)
            .expect("node.input.count");
        count.operands = vec![BinaryOperand::Value(0)];
        let finished = body.blocks[0]
            .instructions
            .as_mut()
            .expect("node instructions")
            .iter_mut()
            .find(|op| op.opcode == 0x5c5)
            .expect("node.finished_cross_group");
        finished.operands = vec![BinaryOperand::Value(0)];
        let errors = verify(body, Some(BinaryTarget::Gpu), "node");
        assert_eq!(
            errors
                .iter()
                .filter(|issue| issue.message.contains("operand count"))
                .count(),
            2,
            "zero-operand node instructions escaped V2: {errors:?}"
        );
    }

    #[test]
    fn v1_multifunction_code_roots_do_not_shift_later_block_parameter_types() {
        let source = "module @NodeRepro tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @NodeRepro.tile() -> ptr<tile> target(gpu) {\nbb0:\n  %position = gpu.spec_const 1, vec<2 x i32>, [1, 2]\n  %address = tile.imageblock.addr %position\n  return %address\n}\n\nfunc module @NodeRepro.graph() -> i1 target(gpu) {\nbb0:\n  %index = gpu.spec_const 2, i32, 0\n  %count = node.input.count\n  %in = node.input.addr %index\n  %alloc = node.output.allocate %index, %count [group]\n  br bb1(%alloc, %index)\nbb1(%rec: ptr<node_record>, %i: i32):\n  %addr = node.output.addr %rec, %i\n  node.output.commit %rec\n  %finished = node.finished_cross_group\n  return %finished\n}\n";
        let parsed = parse_text_module(source).expect("multi-function node source parses");
        let bytes = assemble_text_module(&parsed).expect("multi-function node CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("multi-function node TIRB decodes");
        let errors = verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "CODE-local types from an earlier function shifted block parameter indexes: {errors:?}"
        );
        let canonical = disassemble_text_module(&binary, u64::MAX)
            .expect("multi-function node CODE disassembles");
        let rebuilt = assemble_text_module(
            &parse_text_module(&canonical).expect("canonical multi-function CODE parses"),
        )
        .expect("canonical multi-function CODE reassembles");
        assert_eq!(
            bytes, rebuilt,
            "multi-function TYPES and CODE indices changed"
        );
    }
}
