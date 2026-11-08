use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @MeshCode tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("mesh text parses");
    let bytes = assemble_text_module(&parsed).expect("mesh CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("mesh TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("mesh CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical mesh text parses"))
            .expect("mesh CODE reassembles");
    assert_eq!(bytes, rebuilt, "mesh CODE binary/text/binary differs");
    canonical
}

#[test]
fn all_five_object_and_mesh_opcodes_round_trip() {
    let source = module(
        "func module @MeshCode.object() -> unit _TARGET_ {\nbb0:\n  %x = gpu.spec_const 1, i32, 4\n  %y = gpu.spec_const 2, i32, 5\n  %z = gpu.spec_const 3, i32, 6\n  object.dispatch_mesh %x, %y, %z [unchecked_uniformity]\n  return\n}\n\nfunc module @MeshCode.mesh() -> unit _TARGET_ {\nbb0:\n  %verts = gpu.spec_const 4, i32, 10\n  %prims = gpu.spec_const 5, i32, 6\n  mesh.set_counts %verts, %prims [unchecked_uniformity]\n  mesh.write.vertex %prims, %verts\n  mesh.write.primitive %prims, %verts\n  mesh.write.indices %prims, %verts\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    for name in [
        "object.dispatch_mesh",
        "mesh.set_counts",
        "mesh.write.vertex",
        "mesh.write.primitive",
        "mesh.write.indices",
    ] {
        assert!(
            canonical.contains(name),
            "missing {name} from canonical mesh CODE"
        );
    }
    assert_eq!(canonical.matches("[unchecked_uniformity]").count(), 2);
}

#[test]
fn mesh_indices_accept_vector_i32_and_cross_block_ssa() {
    let source = module(
        "func module @MeshCode.indices() -> unit _TARGET_ {\nbb0:\n  %count = gpu.spec_const 1, i32, 4\n  %indices = const.vec vec<3 x i32>, [0, 1, 2]\n  mesh.set_counts %count, %count\n  br bb1(%count, %indices)\nbb1(%offset: i32, %values: vec<3 x i32>):\n  mesh.write.indices %offset, %values\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("mesh.write.indices"));
    assert!(canonical.contains("mesh.set_counts"));
}

#[test]
fn mesh_indices_reject_four_lane_indices_even_when_ssa_type_matches() {
    let source = module(
        "func module @MeshCode.invalid_lanes() -> unit _TARGET_ {\nbb0:\n  %offset = gpu.spec_const 1, i32, 0\n  %values = const.vec vec<4 x i32>, [0, 1, 2, 3]\n  mesh.write.indices %offset, %values\n  return\n}\n",
        "target(gpu)",
    );
    let parsed = parse_text_module(&source).expect("four-lane index source parses");
    assert!(assemble_text_module(&parsed).is_err());
}

#[test]
fn mesh_rejects_bad_arity_payload_types_and_modifiers() {
    for operation in [
        "object.dispatch_mesh %a, %b",
        "object.dispatch_mesh %a, %b, %c, %a",
        "object.dispatch_mesh %a, %b, %c [unchecked_uniformity] [unchecked_uniformity]",
        "mesh.set_counts %a",
        "mesh.set_counts %a, %b, %c",
        "mesh.write.vertex %a, %b [unchecked_uniformity]",
        "mesh.write.primitive %a, %b [unchecked_uniformity]",
        "mesh.write.indices %a, %b [unchecked_uniformity]",
        "mesh.write.indices %a, %wrong",
        "mesh.write.vertex %wrong, %a",
        "%result = mesh.set_counts %a, %b",
        "%result = object.dispatch_mesh %a, %b, %c",
        "mesh.write.vertex %a, 7",
        "mesh.write.vertex %a, %missing",
    ] {
        let source = module(
            &format!(
                "func module @MeshCode.bad() -> unit _TARGET_ {{\nbb0:\n  %a = gpu.spec_const 1, i32, 4\n  %b = gpu.spec_const 2, i32, 5\n  %c = gpu.spec_const 3, i32, 6\n  %wrong = gpu.spec_const 4, f32, 1.5\n  {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid mesh CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn mesh_object_operations_cannot_encode_cpu_target() {
    for operation in [
        "object.dispatch_mesh %a, %b, %a",
        "mesh.set_counts %a, %b",
        "mesh.write.vertex %a, %b",
        "mesh.write.primitive %a, %b",
        "mesh.write.indices %a, %b",
    ] {
        let source = module(
            &format!(
                "func module @MeshCode.cpu() -> unit _TARGET_ {{\nbb0:\n  %a = const.int i32, 1\n  %b = const.int i32, 2\n  {operation}\n  return\n}}"
            ),
            "engine(native)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "CPU mesh CODE accepted: {operation}"
            );
        }
    }
}
