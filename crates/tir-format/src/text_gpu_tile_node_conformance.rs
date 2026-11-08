use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @WorkGraph tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("tile/node source parses");
    let bytes = assemble_text_module(&parsed).expect("tile/node CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("tile/node TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("tile/node CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical tile/node text parses"),
    )
    .expect("tile/node CODE reassembles");
    assert_eq!(bytes, rebuilt, "tile/node binary/text/binary differs");
    canonical
}

#[test]
fn tile_imageblock_address_uses_vec2_i32_and_ptr_tile() {
    let source = module(
        "func module @WorkGraph.tile() -> ptr<tile> _TARGET_ {\nbb0:\n  %position = gpu.spec_const 1, vec<2 x i32>, [2, 3]\n  %address = tile.imageblock.addr %position\n  return %address\n}\n",
        "target(gpu)",
    );
    assert!(round_trip(&source).contains("tile.imageblock.addr"));
}

#[test]
fn all_six_work_graph_opcodes_round_trip_group_and_thread_mode() {
    let source = module(
        "func module @WorkGraph.node() -> i1 _TARGET_ {\nbb0:\n  %output = gpu.spec_const 1, i32, 0\n  %count = node.input.count\n  %input = node.input.addr %output\n  %alloc = node.output.allocate %output, %count [group]\n  %addr = node.output.addr %alloc, %output\n  node.output.commit %alloc\n  %completed = node.finished_cross_group\n  return %completed\n}\n\nfunc module @WorkGraph.node_default() -> ptr<node_record> _TARGET_ {\nbb0:\n  %output = gpu.spec_const 2, i32, 1\n  %count = gpu.spec_const 3, i32, 4\n  %alloc = node.output.allocate %output, %count [thread]\n  return %alloc\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    for name in [
        "node.input.addr",
        "node.input.count",
        "node.output.allocate",
        "node.output.addr",
        "node.output.commit",
        "node.finished_cross_group",
    ] {
        assert!(canonical.contains(name), "missing {name}");
    }
    assert!(canonical.contains("node.output.allocate %") && canonical.contains(" [group]"));
    assert!(
        !canonical.contains("[thread]"),
        "default mode must be omitted"
    );
}

#[test]
fn node_allocation_and_record_ptr_forward_across_cfg() {
    let source = module(
        "func module @WorkGraph.chain() -> ptr<node_record> _TARGET_ {\nbb0:\n  %output = gpu.spec_const 1, i32, 0\n  %count = gpu.spec_const 2, i32, 4\n  %alloc = node.output.allocate %output, %count [group]\n  br bb1(%alloc)\nbb1(%forwarded: ptr<node_record>):\n  %record = node.output.addr %forwarded, %output\n  node.output.commit %forwarded\n  return %record\n}\n",
        "target(gpu)",
    );
    assert!(round_trip(&source).contains("node.output.commit"));
}

#[test]
fn tile_node_rejects_cpu_arity_duplicate_result_and_incompatible_operands() {
    for (prefix, operation, target) in [
        (
            "%i = const.int i32, 0",
            "%a = tile.imageblock.addr %i",
            "target(gpu)",
        ),
        (
            "%i = const.int i32, 0",
            "%a = tile.imageblock.addr %i",
            "engine(native)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.input.addr",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "node.input.count",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.input.count %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.finished_cross_group %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "node.output.commit %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.output.addr %i, %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.output.allocate %i, %i [all]",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.output.allocate %i, %i [group] [thread]",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.output.allocate %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.output.allocate %i, %i, %i",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.input.addr %undefined",
            "target(gpu)",
        ),
        (
            "%i = gpu.spec_const 1, i32, 0",
            "%a = node.input.addr %i\n  %a = node.input.count",
            "target(gpu)",
        ),
    ] {
        let source = module(
            &format!(
                "func module @WorkGraph.invalid() -> unit _TARGET_ {{\nbb0:\n  {prefix}\n  {operation}\n  return\n}}"
            ),
            target,
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid tile/node CODE assembled: {operation} ({target})"
            );
        }
    }
}

#[test]
fn every_node_opcode_is_gpu_only() {
    for operation in [
        "%a = node.input.addr %i",
        "%a = node.input.count",
        "%a = node.output.allocate %i, %i",
        "%a = node.output.addr %i, %i",
        "node.output.commit %i",
        "%a = node.finished_cross_group",
    ] {
        let source = module(
            &format!(
                "func module @WorkGraph.cpu() -> unit _TARGET_ {{\nbb0:\n  %i = const.int i32, 1\n  {operation}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU node op encoded: {operation}"
            );
        }
    }
}
