use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @Quads tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n")
        .replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("GPU derivative/quad source parses");
    let bytes = assemble_text_module(&parsed).expect("GPU derivative/quad CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU derivative/quad TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("GPU derivative/quad CODE disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical GPU derivative/quad parses"),
    )
    .expect("GPU derivative/quad CODE reassembles");
    assert_eq!(
        bytes, rebuilt,
        "GPU derivative/quad binary round-trip changed bytes"
    );
    canonical
}

#[test]
fn gpu_derivatives_coarse_fine_and_fwidth_round_trip() {
    for operation in [
        "gpu.ddx %input",
        "gpu.ddx %input [coarse]",
        "gpu.ddx %input [fine]",
        "gpu.ddy %input",
        "gpu.ddy %input [fine]",
        "gpu.fwidth %input",
        "gpu.ddx %input [unchecked_uniformity] [fine]",
        "gpu.fwidth %input [unchecked_uniformity]",
    ] {
        let source = module(
            &format!(
                "func module @Quads.diff() -> f32 _TARGET_ {{\nbb0:\n  %input = gpu.spec_const 1, f32, 1.5\n  %out = {operation}\n  return %out\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        let name = operation.split_whitespace().next().unwrap();
        assert!(
            canonical.contains(&format!("= {name} %")),
            "{operation}: {canonical}"
        );
    }
}

#[test]
fn quad_swap_modes_and_broadcast_round_trip() {
    for (operation, result) in [
        ("quad.swap %input", "i32"),
        ("quad.swap %input [x]", "i32"),
        ("quad.swap %input [y]", "i32"),
        ("quad.swap %input [diagonal]", "i32"),
        ("quad.swap %input [unchecked_uniformity] [diagonal]", "i32"),
        ("quad.broadcast %input, %lane", "i32"),
        ("quad.broadcast %input, %lane [unchecked_uniformity]", "i32"),
    ] {
        let source = module(
            &format!(
                "func module @Quads.op() -> {result} _TARGET_ {{\nbb0:\n  %input = gpu.spec_const 1, i32, 42\n  %lane = gpu.spec_const 2, i32, 3\n  %out = {operation}\n  return %out\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains(operation.split_whitespace().next().unwrap()));
    }
}

#[test]
fn quad_any_and_all_accept_boolean_with_interned_result_type() {
    for opcode in ["quad.any", "quad.all"] {
        let source = module(
            &format!(
                "func module @Quads.test() -> i1 _TARGET_ {{\nbb0:\n  %predicate = gpu.spec_const 1, i1, true\n  %out = {opcode} %predicate\n  return %out\n}}"
            ),
            "target(gpu)",
        );
        assert!(round_trip(&source).contains(opcode));
    }
}

#[test]
fn gpu_derivative_vector_and_quad_multiblock_cfg_round_trip() {
    let source = module(
        "func module @Quads.vector() -> vec<3 x f32> _TARGET_ {\nbb0:\n  %vec = gpu.spec_const 1, vec<3 x f32>, [1.5, 2.0, 3.0]\n  %dx = gpu.ddx %vec [fine]\n  br bb1(%dx)\nbb1(%p: vec<3 x f32>):\n  %dy = gpu.ddy %p [fine]\n  %width = gpu.fwidth %dy\n  return %width\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    for opcode in ["gpu.ddx", "gpu.ddy", "gpu.fwidth"] {
        assert!(canonical.contains(opcode));
    }
}

#[test]
fn gpu_derivative_quad_rejects_bad_types_modes_and_ssa_names() {
    for (seed, operation) in [
        ("%input = gpu.spec_const 1, i32, 7", "gpu.ddx %input"),
        ("%input = gpu.spec_const 1, i1, true", "gpu.fwidth %input"),
        ("%input = gpu.spec_const 1, i32, 7", "quad.any %input"),
        (
            "%input = gpu.spec_const 1, f32, 1.5",
            "quad.broadcast %input, %input",
        ),
        ("%input = gpu.spec_const 1, i32, 7", "quad.swap %missing"),
        ("%input = gpu.spec_const 1, i32, 7", "quad.swap %input [z]"),
        (
            "%input = gpu.spec_const 1, i32, 7",
            "quad.swap %input [y] [diagonal]",
        ),
        (
            "%input = gpu.spec_const 1, i32, 7",
            "quad.swap %input [y] [y]",
        ),
        (
            "%input = gpu.spec_const 1, i32, 7",
            "quad.swap %input [fine]",
        ),
        (
            "%input = gpu.spec_const 1, i32, 7",
            "quad.swap %input [unchecked_uniformity] [unchecked_uniformity]",
        ),
        (
            "%input = gpu.spec_const 1, f32, 1.5",
            "gpu.ddx %input [fine] [coarse]",
        ),
        (
            "%input = gpu.spec_const 1, f32, 1.5",
            "gpu.fwidth %input [fine]",
        ),
        (
            "%input = gpu.spec_const 1, f32, 1.5",
            "gpu.ddx %input, %input",
        ),
        (
            "%input = gpu.spec_const 1, i1, true",
            "quad.all %input garbage",
        ),
    ] {
        let source = module(
            &format!(
                "func module @Quads.bad() -> unit _TARGET_ {{\nbb0:\n  {seed}\n  %out = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid GPU quad accepted: {operation}"
            );
        }
    }
}

#[test]
fn gpu_derivative_quad_are_rejected_in_cpu_code() {
    for operation in [
        "gpu.ddx %input",
        "quad.swap %input",
        "quad.broadcast %input, %input",
        "quad.any %input",
    ] {
        let source = module(
            &format!(
                "func module @Quads.bad() -> unit _TARGET_ {{\nbb0:\n  %input = const.int i32, 1\n  %result = {operation}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU quad CODE accepted: {operation}"
            );
        }
    }
}
