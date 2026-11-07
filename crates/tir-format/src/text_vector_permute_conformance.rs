use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @VectorPermute tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("vector text parses");
    let bytes = assemble_text_module(&parsed).expect("vector CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("vector binary decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("vector disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&text).expect("canonical vector text parses"))
            .expect("vector reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn shuffle_uses_lane_indexes_into_concatenated_fixed_vectors() {
    let source = module(
        "func module @VectorPermute.shuffle(%a: vec<2 x i16>, %b: vec<4 x i16>) -> vec<3 x i16> engine(native) {\nbb0(%a: vec<2 x i16>, %b: vec<4 x i16>):\n  %result = vec.shuffle %a, %b, [1, 4, 5]\n  return %result\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("vec.shuffle %arg0, %arg1, [1, 4, 5]"));
}

#[test]
fn integer_and_float_reductions_are_exactly_encoded() {
    for operation in [
        "add", "mul", "and", "or", "xor", "smin", "smax", "umin", "umax", "fadd", "fmul", "fmin",
        "fmax", "fminnum", "fmaxnum",
    ] {
        let float = operation.starts_with('f');
        let (ty, vector) = if float {
            ("f32", "vec<4 x f32>")
        } else {
            ("i32", "vec<4 x i32>")
        };
        let source = module(&format!(
            "func module @VectorPermute.reduce(%values: {vector}, %mask: vec<4 x i1>) -> {ty} engine(native) {{\nbb0(%values: {vector}, %mask: vec<4 x i1>):\n  %result = vec.reduce {operation} %values [mask %mask]\n  return %result\n}}"
        ));
        let text = round_trip(&source);
        assert!(text.contains(&format!("vec.reduce {operation} %arg0 [mask %arg1]")));
    }
}

#[test]
fn unmasked_float_reassociation_is_preserved_in_round_trip() {
    for operation in ["fadd", "fmul"] {
        let source = module(&format!(
            "func module @VectorPermute.reassoc(%values: vec<8 x f64>) -> f64 engine(native) {{\nbb0(%values: vec<8 x f64>):\n  %result = vec.reduce [reassoc] {operation} %values\n  return %result\n}}"
        ));
        assert!(round_trip(&source).contains(&format!("vec.reduce [reassoc] {operation} %arg0")));
    }
}

#[test]
fn shuffle_and_reduce_can_chain_through_a_successor_block() {
    let source = module(
        "func module @VectorPermute.cfg(%a: vec<4 x i32>, %b: vec<4 x i32>) -> i32 engine(native) {\nbb0(%a: vec<4 x i32>, %b: vec<4 x i32>):\n  %shuffled = vec.shuffle %a, %b, [0, 6, 1, 7]\n  br bb1(%shuffled)\nbb1(%v: vec<4 x i32>):\n  %sum = vec.reduce add %v\n  return %sum\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("vec.shuffle"));
    assert!(text.contains("vec.reduce add"));
}

#[test]
fn shuffle_and_reduce_reject_invalid_indices_types_and_modifiers() {
    for (params, expression) in [
        (
            "%a: vec<4 x i32>, %b: vec<4 x i32>",
            "%result = vec.shuffle %a, %b, [-1, 2]",
        ),
        (
            "%a: vec<4 x i32>, %b: vec<4 x i32>",
            "%result = vec.shuffle %a, %b, [0, 8]",
        ),
        (
            "%a: vec<4 x i32>, %b: vec<2 x i64>",
            "%result = vec.shuffle %a, %b, [0, 1]",
        ),
        (
            "%a: vec<4 x i32>, %b: vec<4 x i32>",
            "%result = vec.shuffle %a, %b, [0]",
        ),
        (
            "%a: vec<4 x i32>, %b: vec<4 x i32>",
            "%result = vec.shuffle %a, %b, [0, 1.5]",
        ),
        ("%a: vec<4 x f32>", "%result = vec.reduce add %a"),
        ("%a: vec<4 x i32>", "%result = vec.reduce fadd %a"),
        ("%a: vec<4 x i32>", "%result = vec.reduce [reassoc] add %a"),
        (
            "%a: vec<4 x i32>, %m: vec<2 x i1>",
            "%result = vec.reduce add %a [mask %m]",
        ),
        ("%a: vec<4 x i32>", "%result = vec.reduce add %missing"),
    ] {
        let source = module(&format!(
            "func module @VectorPermute.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {expression}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid vector CODE accepted: {expression}"
            );
        }
    }
}
