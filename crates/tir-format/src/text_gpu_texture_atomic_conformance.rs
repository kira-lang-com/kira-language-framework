use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @AtomicTextures tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("texture atomic source parses");
    let bytes = assemble_text_module(&parsed).expect("texture atomic CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("texture atomic TIRB decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("texture atomic disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical texture atomic text parses"),
    )
    .expect("texture atomic reassembles");
    assert_eq!(
        bytes, rebuilt,
        "texture atomic binary/text/binary changed bytes"
    );
    canonical
}

#[test]
fn texture_atomic_all_fifteen_operation_codes_round_trip() {
    for (index, name) in [
        "xchg", "add", "sub", "and", "nand", "or", "xor", "max", "min", "umax", "umin", "fadd",
        "fsub", "fmax", "fmin",
    ]
    .iter()
    .enumerate()
    {
        let (ty, default) = if index >= 11 {
            ("f32", "1.5")
        } else {
            ("i32", "7")
        };
        let source = module(
            &format!(
                "func module @AtomicTextures.atomic(%tex: texture<2d, {ty}, readwrite>, %coords: vec<2 x i32>, %value: {ty}) -> {ty} _TARGET_ {{\nbb0(%tex: texture<2d, {ty}, readwrite>, %coords: vec<2 x i32>, %value: {ty}):\n  %old = tex.atomic %tex, %coords, {name} %value\n  return %old\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(
            canonical.contains(&format!("tex.atomic %arg0, %arg1, {name} %arg2")),
            "operation {name} with value {default} was not canonicalized: {canonical}"
        );
    }
}

#[test]
fn texture_atomic_array_coordinates_and_multi_block_arguments_round_trip() {
    for (dimension, coordinate, layer) in [
        ("1d", "i32", ""),
        ("1darray", "i32", ", %layer"),
        ("2darray", "vec<2 x i32>", ", %layer"),
        ("3d", "vec<3 x i32>", ""),
        ("cube", "vec<3 x i32>", ""),
        ("cubearray", "vec<3 x i32>", ", %layer"),
    ] {
        let source = module(
            &format!(
                "func module @AtomicTextures.array(%tex: texture<{dimension}, i32, readwrite>, %coords: {coordinate}, %layer: i32, %value: i32) -> i32 _TARGET_ {{\nbb0(%tex: texture<{dimension}, i32, readwrite>, %coords: {coordinate}, %layer: i32, %value: i32):\n  %old = tex.atomic %tex, %coords{layer}, add %value\n  br bb1(%old)\nbb1(%previous: i32):\n  return %previous\n}}"
            ),
            "target(gpu)",
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains("tex.atomic"));
    }
}

#[test]
fn texture_atomic_rejects_malformed_types_modes_arity_and_operations() {
    for (texture, coords, value, operation) in [
        (
            "texture<2d, i32, read>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, i32, write>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, i32, readwrite, ms>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, vec<4 x i32>, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, f32, readwrite>",
            "vec<2 x i32>",
            "f32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, fadd %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, banana %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %missing",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x f32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<3 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, add %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, %value",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value [sparse]",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value offset [0, 0]",
        ),
        (
            "texture<2d, i32, readwrite>",
            "vec<2 x i32>",
            "i32",
            "tex.atomic %tex, %coords, add %value [unchecked_uniformity]",
        ),
    ] {
        let source = module(
            &format!(
                "func module @AtomicTextures.invalid(%tex: {texture}, %coords: {coords}, %value: {value}) -> unit _TARGET_ {{\nbb0(%tex: {texture}, %coords: {coords}, %value: {value}):\n  %old = {operation}\n  return\n}}"
            ),
            "target(gpu)",
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid atomic texture CODE accepted: {operation}"
            );
        }
    }
}

#[test]
fn texture_atomic_gpu_target_only() {
    let source = module(
        "func module @AtomicTextures.cpu(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>, %value: i32) -> i32 _TARGET_ {\nbb0(%tex: texture<2d, i32, readwrite>, %coords: vec<2 x i32>, %value: i32):\n  %old = tex.atomic %tex, %coords, add %value\n  return %old\n}\n",
        "engine(native)",
    );
    if let Ok(parsed) = parse_text_module(&source) {
        assert!(
            assemble_text_module(&parsed).is_err(),
            "CPU texture atomic incorrectly assembled"
        );
    }
}
