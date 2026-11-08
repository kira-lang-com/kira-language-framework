use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @Subgroups tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("subgroup source parses");
    let bytes = assemble_text_module(&parsed).expect("subgroup CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("subgroup TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("subgroup CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical subgroup source parses"),
    )
    .expect("subgroup CODE reassembles");
    assert_eq!(bytes, rebuilt, "subgroup binary/text/binary changed bytes");
    canonical
}

#[test]
fn subgroup_all_nineteen_collective_opcodes_round_trip() {
    let source = module(
        "func module @Subgroups.all() -> i32 _TARGET_ {\nbb0:\n  %bool = gpu.spec_const 1, i1, true\n  %data = gpu.spec_const 2, i32, 42\n  %lane = gpu.spec_const 3, i32, 0\n  %elect = subgroup.elect\n  %all = subgroup.all %bool\n  %any = subgroup.any %bool\n  %equal = subgroup.all_equal %data\n  %broadcast = subgroup.broadcast %data, %lane\n  %first = subgroup.broadcast_first %broadcast\n  %ballot = subgroup.ballot %bool\n  %inverse = subgroup.inverse_ballot %ballot\n  %count = subgroup.ballot_bit_count %ballot\n  %lsb = subgroup.ballot_find_lsb %ballot\n  %msb = subgroup.ballot_find_msb %ballot\n  %shuf = subgroup.shuffle %first, %lane\n  %xor = subgroup.shuffle_xor %shuf, %lane\n  %up = subgroup.shuffle_up %xor, %lane\n  %down = subgroup.shuffle_down %up, %lane\n  %rotate = subgroup.rotate %down, %lane\n  %reduced = subgroup.reduce %rotate [add.s] [cluster 4]\n  %inclusive = subgroup.scan_inclusive %reduced [min.s]\n  %exclusive = subgroup.scan_exclusive %inclusive [xor] [cluster 8]\n  return %exclusive\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    for name in crate::text_gpu_subgroup::NAMES {
        assert!(
            canonical.contains(name),
            "{name} missing from canonical subgroup CODE"
        );
    }
    assert!(canonical.contains("subgroup.reduce %") && canonical.contains("[add.s] [cluster 4]"));
    assert!(
        canonical.contains("subgroup.scan_exclusive %") && canonical.contains("[xor] [cluster 8]")
    );
}

#[test]
fn subgroup_every_reduction_operator_and_cluster_size_round_trip() {
    for (index, name) in crate::text_gpu_subgroup::OPERATORS.iter().enumerate() {
        let float = matches!(index, 2 | 5 | 8 | 11);
        let (ty, value) = if float { ("f32", "1.5") } else { ("i32", "7") };
        let size = if index % 2 == 0 { "128" } else { "1" };
        for op in [
            "subgroup.reduce",
            "subgroup.scan_inclusive",
            "subgroup.scan_exclusive",
        ] {
            let source = module(
                &format!(
                    "func module @Subgroups.reduce() -> {ty} _TARGET_ {{\nbb0:\n  %value = gpu.spec_const 1, {ty}, {value}\n  %result = {op} %value [{name}] [cluster {size}]\n  return %result\n}}"
                ),
                "target(gpu)",
            );
            let canonical = round_trip(&source);
            assert!(
                canonical.contains(&format!("[{name}] [cluster {size}]")),
                "{op}: {canonical}"
            );
        }
    }
}

#[test]
fn subgroup_ballot_queries_and_bit_predicates_can_chain_with_cfg() {
    let source = module(
        "func module @Subgroups.cfg() -> i32 _TARGET_ {\nbb0:\n  %predicate = gpu.spec_const 1, i1, true\n  %mask = subgroup.ballot %predicate\n  br bb1(%mask)\nbb1(%incoming: vec<4 x i32>):\n  %population = subgroup.ballot_bit_count %incoming\n  %first = subgroup.ballot_find_lsb %incoming\n  %result = add %population, %first\n  return %result\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("subgroup.ballot_bit_count"));
    assert!(canonical.contains("subgroup.ballot_find_lsb"));
}

#[test]
fn subgroup_rejects_bad_arity_scalar_types_and_operators() {
    for (seed, instruction) in [
        ("%value = gpu.spec_const 1, i32, 7", "subgroup.elect %value"),
        ("%value = gpu.spec_const 1, i32, 7", "subgroup.all %value"),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.ballot %value",
        ),
        (
            "%value = gpu.spec_const 1, i1, true",
            "subgroup.ballot_bit_count %value",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.broadcast %value",
        ),
        (
            "%value = gpu.spec_const 1, i1, true",
            "subgroup.shuffle %value, %value",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.f]",
        ),
        (
            "%value = gpu.spec_const 1, f32, 1.5",
            "subgroup.reduce %value [add.s]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [unknown]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.s] [xor]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.s] [cluster 0]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.s] [cluster 3]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.s] [cluster 256]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %value [add.s] [cluster 4] [cluster 8]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.broadcast_first %value [add.s]",
        ),
        (
            "%value = gpu.spec_const 1, i32, 7",
            "subgroup.reduce %missing [add.s]",
        ),
    ] {
        let source = module(
            &format!(
                "func module @Subgroups.invalid() -> unit _TARGET_ {{\nbb0:\n  {seed}\n  %result = {instruction}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid subgroup accepted: {instruction}"
            );
        }
    }
}

#[test]
fn subgroup_operations_are_gpu_only() {
    for instruction in [
        "subgroup.elect",
        "subgroup.all %value",
        "subgroup.reduce %value [add.s]",
        "subgroup.broadcast_first %value",
    ] {
        let source = module(
            &format!(
                "func module @Subgroups.cpu() -> unit _TARGET_ {{\nbb0:\n  %value = const.int i32, 1\n  %result = {instruction}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU subgroup accepted: {instruction}"
            );
        }
    }
}
