//! V1/V2/V4/V19 cross-section, heap and reference conformance tests.
use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str, profile: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile {profile} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn v1_v2_switch_enum_checks_exhaustiveness_and_implicit_payload() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = format!(
        "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\ntype %Choice = enum ridl {{ None, Number(i32) }} policy default unmanaged\n\n{}\n",
        "func module @Smoke.dispatch(%value: %Choice) -> i32 engine(native) {\nbb0(%value: %Choice):\n  switch.enum %value, [None: bb2], [Number: bb1]\nbb1(%payload: i32):\n  return %payload\nbb2:\n  %zero = const.int i32, 0\n  return %zero\n}"
    );
    let parsed = parse_text_module(&source).expect("enum source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V1/V2 text rejects enum switch: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("enum switch assembles"))
            .expect("enum switch binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V1/V2 binary rejects enum switch: {binary_errors:?}"
    );
}

#[test]
fn v2_v19_function_reference_feeds_matching_indirect_call() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.main(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  %target = func.ref @Smoke.identity\n  %result = call.indirect %target : fn(i32) -> i32 (%arg)\n  return %result\n}\n\nfunc module @Smoke.identity(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  return %arg\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("func.ref text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "func.ref text failed V2/V19: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("func.ref CODE assembles"))
            .expect("func.ref binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "func.ref binary failed V2/V19: {errors:?}"
    );
}

#[test]
fn v2_global_tls_and_descriptor_references_are_validated_cross_section() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "global module @Smoke.shared: i128 = 1\nglobal module @Smoke.thread: i32 = 3 thread_local\n\nfunc module @Smoke.references() -> ptr engine(native) {\nbb0:\n  %shared = global.addr @Smoke.shared\n  %thread = tls.addr @Smoke.thread\n  %desc = descriptor.ref (ptr, i64)\n  return %desc\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("reference CODE text parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "reference CODE text V2 errors: {errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("reference CODE assembles"))
            .expect("reference binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "reference CODE binary V2 errors: {errors:?}"
    );
}

#[test]
fn v2_v4_counted_and_unique_heap_allocations_are_consumed_exactly_once() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.manage() -> unit engine(native) {\nbb0:\n  %counted = heap.alloc.counted i32\n  %unique = heap.alloc.unique i64\n  destroy %counted\n  destroy %unique\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("managed heap text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2/V4 rejects owned heap cleanup: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("managed heap CODE assembles"))
            .expect("managed heap CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2/V4 rejects owned heap cleanup: {errors:?}"
    );
}

#[test]
fn v2_heap_borrowed_payload_address_and_uniqueness_are_typed() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};
    let source = module(
        "func module @Smoke.inspect(%shared: borrow ref i32, %unique: borrow uniq i64) -> i1 engine(native) {\nbb0(%shared: borrow ref i32, %unique: borrow uniq i64):\n  %shared_addr = ref.addr %shared\n  %unique_addr = uniq.addr %unique\n  %raw = ref.unowned %shared\n  %exclusive = ref.isunique %shared\n  return %exclusive\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("reference access text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects borrowed heap inspection: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("reference access CODE assembles"),
    )
    .expect("reference access CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects borrowed heap inspection: {errors:?}"
    );
}

#[test]
fn v2_v4_weak_new_and_weak_load_require_counted_optional_lifecycle() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Optional<$T> = enum ridl { None, Some($T) } policy { copy: implicit(@Smoke.optional_copy), destroy: fn(@Smoke.optional_destroy), relocate: bitwise, exclusivity: static, pairing: counted, lifetime: eager }\n\nfunc package @Smoke.optional_copy(%dst: ptr, %src: ptr, %type: ptr) -> unit engine(native)\nfunc package @Smoke.optional_destroy(%object: ptr, %type: ptr) -> unit engine(native)\n\nfunc module @Smoke.create(%strong: borrow ref i32) -> weak i32 engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  return %weak\n}\n\nfunc module @Smoke.promote(%weak: borrow weak i32) -> %Optional<ref i32> engine(native) {\nbb0(%weak: borrow weak i32):\n  %result = weak.load %weak\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("typed weak reference text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2/V4 text rejects owned weak lifetimes: {text_errors:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("weak CODE assembles"))
        .expect("weak CODE binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V2/V4 binary rejects owned weak lifetimes: {binary_errors:?}"
    );
}

#[test]
fn v4_managed_allocations_and_new_weak_references_must_be_consumed() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    for operation in [
        "%object = heap.alloc.counted i32",
        "%object = heap.alloc.unique i64",
        "%object = weak.new %strong",
    ] {
        let source = module(
            &format!(
                "func module @Smoke.leak(%strong: borrow ref i32) -> unit engine(native) {{\nbb0(%strong: borrow ref i32):\n  {operation}\n  return\n}}"
            ),
            "app",
        );
        let parsed = parse_text_module(&source).expect("leaking managed heap text parses");
        let text_errors = verify_text_module(&parsed, Default::default());
        assert!(
            text_errors
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0004"),
            "text V4 accepted leaked managed heap value from {operation}: {text_errors:?}"
        );
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("raw leaking CODE still assembles"),
        )
        .expect("leaking CODE binary decodes");
        let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            binary_errors
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0004"),
            "binary V4 accepted leaked managed heap value from {operation}: {binary_errors:?}"
        );
    }
}

#[test]
fn v5_trivial_borrow_return_is_rejected_in_text_and_binary() {
    use tir_format::{BinaryModule, assemble_text_module};
    for body in [
        "func module @Smoke.invalid(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  %loan = borrow.begin %arg\n  borrow.end %loan\n  return %loan\n}",
        "func module @Smoke.invalid(%arg: borrow i32) -> i32 engine(native) {\nbb0(%arg: borrow i32):\n  return %arg\n}",
    ] {
        let parsed = parse_text_module(&module(body, "app")).expect("borrowed value syntax parses");
        let text_errors = super::verify_text_module(&parsed, Default::default());
        assert!(
            text_errors
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0005"),
            "text V5 accepted a returned trivial borrow: {text_errors:?}"
        );
        let binary = BinaryModule::decode(
            &assemble_text_module(&parsed).expect("ill-typed CODE can still be encoded"),
        )
        .expect("borrowed return binary decodes");
        let binary_errors = super::verify_binary_module_records(&binary, u64::MAX);
        assert!(
            binary_errors
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0005"),
            "binary V5 accepted a returned trivial borrow: {binary_errors:?}"
        );
    }
}

#[test]
fn v2_shuffle_and_reductions_have_matching_text_and_binary_types() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.permute(%a: vec<4 x i32>, %b: vec<4 x i32>, %mask: vec<4 x i1>) -> i32 engine(native) {\nbb0(%a: vec<4 x i32>, %b: vec<4 x i32>, %mask: vec<4 x i1>):\n  %mixed = vec.shuffle %a, %b, [0, 7, 2, 5]\n  %sum = vec.reduce add %mixed [mask %mask]\n  return %sum\n}\n\nfunc module @Smoke.float(%values: vec<8 x f64>) -> f64 engine(native) {\nbb0(%values: vec<8 x f64>):\n  %result = vec.reduce [reassoc] fmul %values\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("vector shuffle/reduce parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects shuffle/reduce: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("vector CODE assembles"))
            .expect("vector CODE decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects shuffle/reduce: {binary_errors:?}"
    );
}

#[test]
fn v2_compress_expand_slide_and_table_have_typed_binary_results() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.combine(%a: vec<4 x i32>, %b: vec<4 x i32>, %mask: vec<4 x i1>, %offset: i32) -> vec<4 x i32> engine(native) {\nbb0(%a: vec<4 x i32>, %b: vec<4 x i32>, %mask: vec<4 x i1>, %offset: i32):\n  %packed = vec.compress %a, %mask, %b\n  %expanded = vec.expand %packed, %mask, %a\n  %shift = vec.slide %expanded, %b, %offset [down]\n  return %shift\n}\n\nfunc module @Smoke.lookup(%table: vec<8 x i8>, %indexes: vec<4 x i32>) -> vec<4 x i8> engine(native) {\nbb0(%table: vec<8 x i8>, %indexes: vec<4 x i32>):\n  %bytes = vec.table %table, %indexes\n  return %bytes\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("vector compression and table source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects vector operations: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("vector CODE assembles"))
            .expect("vector CODE decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects vector operations: {binary_errors:?}"
    );
}

#[test]
fn v2_vector_dot_kinds_preserve_integer_and_bf16_shapes() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.ints(%acc: vec<4 x i32>, %a: vec<16 x i8>, %b: vec<16 x i8>) -> vec<4 x i32> engine(native) {\nbb0(%acc: vec<4 x i32>, %a: vec<16 x i8>, %b: vec<16 x i8>):\n  %result = vec.dot usdot %acc, %a, %b\n  return %result\n}\n\nfunc module @Smoke.float(%acc: vec<4 x f32>, %a: vec<8 x bf16>, %b: vec<8 x bf16>) -> vec<4 x f32> engine(native) {\nbb0(%acc: vec<4 x f32>, %a: vec<8 x bf16>, %b: vec<8 x bf16>):\n  %result = vec.dot bfdot %acc, %a, %b\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("vec.dot source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects dot: {text_errors:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("dot CODE assembles"))
        .expect("dot CODE decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects dot: {binary_errors:?}"
    );
}

#[test]
fn v2_vector_load_store_masked_and_unmasked_are_typed() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.unmasked(%source: ptr, %dest: ptr) -> vec<4 x i32> engine(native) {\nbb0(%source: ptr, %dest: ptr):\n  %v = vec.load vec<4 x i32>, %source [align 16]\n  vec.store %v, %dest [align 16]\n  return %v\n}\n\nfunc module @Smoke.masked(%ptr: ptr, %mask: vec<4 x i1>, %old: vec<4 x f32>) -> vec<4 x f32> engine(native) {\nbb0(%ptr: ptr, %mask: vec<4 x i1>, %old: vec<4 x f32>):\n  %loaded = vec.load vec<4 x f32>, %ptr [align 16] [mask %mask, passthru %old]\n  vec.store %loaded, %ptr [mask %mask]\n  return %loaded\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("vector memory source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects typed vector load/store: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("vector memory CODE assembles"))
            .expect("vector memory binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects typed vector load/store: {binary_errors:?}"
    );
}

#[test]
fn v2_gather_scatter_pointer_vector_and_base_scaled_forms_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.pointers(%ptrs: vec<4 x ptr>, %mask: vec<4 x i1>, %old: vec<4 x i32>) -> vec<4 x i32> engine(native) {\nbb0(%ptrs: vec<4 x ptr>, %mask: vec<4 x i1>, %old: vec<4 x i32>):\n  %gathered = vec.gather vec<4 x i32>, %ptrs [mask %mask, passthru %old]\n  vec.scatter %gathered, %ptrs [mask %mask]\n  return %gathered\n}\n\nfunc module @Smoke.scaled(%base: ptr, %offsets: vec<4 x i32>, %mask: vec<4 x i1>, %old: vec<4 x f32>) -> vec<4 x f32> engine(native) {\nbb0(%base: ptr, %offsets: vec<4 x i32>, %mask: vec<4 x i1>, %old: vec<4 x f32>):\n  %gathered = vec.gather vec<4 x f32>, %base, %offsets, 4 [mask %mask, passthru %old]\n  vec.scatter %gathered, %base, %offsets, 4 [mask %mask]\n  return %gathered\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("gather/scatter source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects gather/scatter: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("gather/scatter CODE assembles"),
    )
    .expect("gather/scatter binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects gather/scatter: {binary_errors:?}"
    );
}

#[test]
fn v2_trivial_nominal_struct_and_enum_construction_verify_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Record = struct ridl { first: i32, second: i64 } policy default unmanaged\ntype %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Smoke.record(%a: i32, %b: i64) -> %Record engine(native) {\nbb0(%a: i32, %b: i64):\n  %new = struct.new %Record(%a, %b)\n  %changed = struct.insert %new, 0, %a\n  return %changed\n}\n\nfunc module @Smoke.enum(%a: i32) -> %Choice engine(native) {\nbb0(%a: i32):\n  %choice = enum.new %Choice, Number(%a)\n  return %choice\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("nominal CODE source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2 rejects nominal source: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("nominal CODE assembles"))
            .expect("nominal binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V2 rejects nominal binary: {binary_errors:?}"
    );
}

#[test]
fn v2_ridl_enum_tag_result_is_i8_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Smoke.tag(%choice: %Choice) -> i8 engine(native) {\nbb0(%choice: %Choice):\n  %tag = enum.tag %choice\n  return %tag\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("enum.tag source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects enum.tag: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("enum.tag CODE assembles"))
            .expect("enum.tag binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects enum.tag: {binary_errors:?}"
    );
}

#[test]
fn v2_c_and_packed_enum_tags_have_specified_widths() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %CChoice = enum c { None, Some } policy default unmanaged\ntype %PackedChoice = enum packed(1) { None, Some(i32) } policy default unmanaged\n\nfunc module @Smoke.c_tag(%v: %CChoice) -> i32 engine(native) {\nbb0(%v: %CChoice):\n  %tag = enum.tag %v\n  return %tag\n}\n\nfunc module @Smoke.packed_tag(%v: %PackedChoice) -> i8 engine(native) {\nbb0(%v: %PackedChoice):\n  %tag = enum.tag %v\n  return %tag\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("C/packed enum source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "text V2 rejects enum tag types: {errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("C/packed enum tag CODE assembles"),
    )
    .expect("C/packed tag CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects enum tag types: {errors:?}"
    );
}

#[test]
fn v2_struct_extract_uses_declared_field_type() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Pair = struct ridl { number: i64, ratio: f32 } policy default unmanaged\n\nfunc module @Smoke.extract(%pair: %Pair) -> f32 engine(native) {\nbb0(%pair: %Pair):\n  %ratio = struct.extract %pair, 1\n  return %ratio\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("struct.extract parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "text V2 rejects struct.extract: {errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("struct.extract CODE assembles"),
    )
    .expect("struct.extract CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects struct.extract: {errors:?}"
    );
}

#[test]
fn v2_v4_trivial_struct_and_tuple_destructure_produce_typed_multiple_results() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Pair = struct ridl { x: i32, y: i32 } policy default unmanaged\n\nfunc module @Smoke.struct(%record: %Pair) -> i32 engine(native) {\nbb0(%record: %Pair):\n  %a, %b = struct.destructure %record\n  %sum = add %a, %b\n  return %sum\n}\n\nfunc module @Smoke.tuple(%numbers: (i64, i64)) -> i64 engine(native) {\nbb0(%numbers: (i64, i64)):\n  %left, %right = tuple.destructure %numbers\n  %sum = add %left, %right\n  return %sum\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("aggregate destructure source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2/V4 text rejects aggregate destructure: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("aggregate destructure CODE assembles"),
    )
    .expect("aggregate destructure TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V2/V4 binary rejects aggregate destructure: {binary_errors:?}"
    );
}

#[test]
fn v2_v4_trivial_enum_take_and_borrowed_project_verify_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Choice = enum ridl { None, Number(i32), Pair(i32, i64) } policy default unmanaged\n\nfunc module @Smoke.take(%value: %Choice) -> i32 engine(native) {\nbb0(%value: %Choice):\n  %number = enum.take %value, Number\n  return %number\n}\n\nfunc module @Smoke.project(%value: borrow %Choice) -> unit engine(native) {\nbb0(%value: borrow %Choice):\n  %number = enum.project %value, Number\n  return\n}\n\nfunc module @Smoke.take_pair(%value: %Choice) -> i64 engine(native) {\nbb0(%value: %Choice):\n  %payload = enum.take %value, Pair\n  %number = tuple.extract %payload, 1\n  return %number\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("enum payload source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2/V4 text rejects enum payload: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("enum payload CODE assembles"))
            .expect("enum payload binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V2/V4 binary rejects enum payload: {binary_errors:?}"
    );
}

#[test]
fn v2_v5_enum_project_requires_borrowed_source() {
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Smoke.invalid(%value: %Choice) -> unit engine(native) {\nbb0(%value: %Choice):\n  %number = enum.project %value, Number\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("invalid enum.project parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|issue| issue.severity == tir_diag::Severity::Error),
        "enum.project on a nonborrowed enum escaped verification: {diagnostics:?}"
    );
}
