use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @Builtins tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n")
        .replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("GPU builtin text parses");
    let bytes = assemble_text_module(&parsed).expect("GPU builtin CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU builtin TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("GPU builtin CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("GPU builtin canonical text parses"),
    )
    .expect("GPU builtin CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn gpu_builtin_scalar_and_vector_names_round_trip() {
    for (name, ty) in [
        ("lane_index", "i32"),
        ("thread_position_in_grid", "vec<3 x i32>"),
        ("position", "vec<4 x f32>"),
        ("front_facing", "i1"),
        ("point_coord", "vec<2 x f32>"),
        ("shading_rate", "vec<2 x i32>"),
    ] {
        let source = module(
            &format!(
                "func module @Builtins.read() -> {ty} _TARGET_ {{\nbb0:\n  %value = gpu.builtin {name} : {ty}\n  return %value\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains(&format!("gpu.builtin {name} : {ty}")));
    }
}

#[test]
fn gpu_builtin_interns_names_across_multiple_functions_and_blocks() {
    let source = module(
        "func module @Builtins.first() -> i32 _TARGET_ {\nbb0:\n  %a = gpu.builtin lane_index : i32\n  %b = gpu.builtin subgroup_size : i32\n  %sum = add %a, %b\n  br bb1(%sum)\nbb1(%index: i32):\n  return %index\n}\n\nfunc module @Builtins.second() -> i32 _TARGET_ {\nbb0:\n  %value = gpu.builtin lane_index : i32\n  return %value\n}",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("gpu.builtin lane_index").count(), 2);
    assert_eq!(canonical.matches("gpu.builtin subgroup_size").count(), 1);
}

#[test]
fn gpu_builtin_rejects_wrong_name_shape_and_cpu_target() {
    for (name, ty, target) in [
        ("does_not_exist", "i32", "target(gpu)"),
        ("lane_index", "i1", "target(gpu)"),
        ("position", "vec<3 x f32>", "target(gpu)"),
        ("front_facing", "i32", "target(gpu)"),
        ("thread_position_in_grid", "i32", "target(gpu)"),
        ("lane_index", "i32", "engine(native)"),
    ] {
        let source = module(
            &format!(
                "func module @Builtins.bad() -> {ty} _TARGET_ {{\nbb0:\n  %value = gpu.builtin {name} : {ty}\n  return %value\n}}"
            ),
            target,
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid GPU builtin accepted: {name} : {ty} {target}"
            );
        }
    }
}

#[test]
fn gpu_builtin_rejects_undefined_ssa_result_and_duplicate_defs() {
    for body in [
        "  gpu.builtin lane_index : i32\n  return",
        "  %a, %b = gpu.builtin lane_index : i32\n  return",
        "  %a = gpu.builtin lane_index : i32\n  %a = gpu.builtin subgroup_size : i32\n  return %a",
    ] {
        let source = module(
            &format!("func module @Builtins.bad() -> unit _TARGET_ {{\nbb0:\n{body}\n}}"),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid GPU builtin accepted: {body}"
            );
        }
    }
}
