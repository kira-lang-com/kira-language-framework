//! Additional binary CODE V2/V14/V20 conformance for newer instruction families.
use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str, profile: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile {profile} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn v2_v20_mmio_load_store_and_fence_verify_for_tessera_driver() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Smoke tir 26.11.1 profile driver platform tessera semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n  source \"0000000000000000000000000000000000000000000000000000000000000000\"\n\nfunc module @Smoke.io(%p: ptr<device>, %value: i32) -> i32 engine(native) nounwind {\nbb0(%p: ptr<device>, %value: i32):\n  fence.io [write]\n  mmio.store %value, %p [order release] [align 4]\n  %loaded = mmio.load i32, %p [order acquire] [align 4]\n  fence.io [full]\n  return %loaded\n}\n";
    let parsed = parse_text_module(source).expect("device text parses");
    let bytes = assemble_text_module(&parsed).expect("device CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("device binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_eq!(analysis.functions[0].reason_mask & 0x80, 0x80);
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "valid MMIO CODE has verifier errors: {errors:?}"
    );
}

#[test]
fn v2_v20_raw_heap_allocation_reallocation_and_free_are_typed_and_unmanaged() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.raw(%size: i64, %align: i64) -> unit engine(native) {\nbb0(%size: i64, %align: i64):\n  %pointer = heap.alloc.raw %size, %align\n  %resized = heap.realloc.raw %pointer, %size, %align\n  heap.free.raw %resized\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("heap source parses");
    let bytes = assemble_text_module(&parsed).expect("heap CODE assembles");
    let module = BinaryModule::decode(&bytes).expect("heap binary decodes");
    let analysis = module.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_ne!(analysis.functions[0].reason_mask & 0x02, 0);
    let errors = super::verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2/V20 rejects valid raw heap CODE: {errors:?}"
    );
}

#[test]
fn v14_uninitialized_raw_heap_requires_permissive_semantics() {
    use tir_format::{BinaryModule, assemble_text_module};

    let body = "func module @Smoke.heap(%size: i64, %align: i64) -> ptr engine(native) {\nbb0(%size: i64, %align: i64):\n  %pointer = heap.alloc.raw %size, %align [uninit]\n  return %pointer\n}";
    for (mode, must_fail) in [("defined", true), ("permissive", false)] {
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
        );
        let parsed = parse_text_module(&source).expect("uninit raw heap text parses");
        let text_issues = verify_text_module(&parsed, Default::default());
        assert_eq!(
            text_issues
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            must_fail,
            "text V14 mismatch for {mode}: {text_issues:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("uninit raw heap CODE assembles");
        let module = BinaryModule::decode(&bytes).expect("uninit raw heap binary decodes");
        let diagnostics = super::verify_binary_module_records(&module, u64::MAX);
        assert_eq!(
            diagnostics
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            must_fail,
            "binary V14 mismatch for {mode}: {diagnostics:?}"
        );
    }
}

#[test]
fn v2_float_classification_frexp_ldexp_and_flags_are_well_typed() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.float(%a: f32, %exponent: i32) -> i32 engine(native) {\nbb0(%a: f32, %exponent: i32):\n  %class = fclass %a\n  %parts = frexp %a\n  %mantissa = tuple.extract %parts, 0\n  %reconstructed = ldexp %mantissa, %exponent\n  %flags = fp.flags.read\n  fp.flags.clear\n  return %class\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("float special source parses");
    let bytes = assemble_text_module(&parsed).expect("float special CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("float CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 rejects fclass/frexp/ldexp/flags: {errors:?}"
    );
}

#[test]
fn v2_fp_flags_clear_has_no_text_or_binary_ssa_result() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.flags() -> i32 engine(native) {\nbb0:\n  fp.flags.clear\n  %status = fp.flags.read\n  return %status\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("FP status text parses");
    let text_issues = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_issues.is_empty(),
        "fp.flags.clear incorrectly defines an SSA result: {text_issues:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("FP status CODE encodes");
    let binary = BinaryModule::decode(&bytes).expect("FP status binary decodes");
    let binary_issues = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_issues.is_empty(),
        "FP status binary verification failed: {binary_issues:?}"
    );
}

#[test]
fn v2_cpu_atomic_load_store_and_fence_verify_with_explicit_seq_cst() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.atomic(%ptr: ptr, %value: i64) -> i64 engine(native) {\nbb0(%ptr: ptr, %value: i64):\n  atomic.store [order=release] [scope=system] %value, %ptr\n  %a = atomic.load [order=acquire] [scope=thread] i64, %ptr\n  atomic.store [order=seq_cst] %a, %ptr\n  fence [order=acq_rel] [scope=system]\n  %b = atomic.load [order=seq_cst] i64, %ptr\n  return %b\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("atomic source parses");
    let encoded = assemble_text_module(&parsed).expect("atomic CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("atomic CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "valid CPU atomics cause verifier errors: {errors:?}"
    );
}

#[test]
fn v2_atomic_wait_and_notify_use_runtime_compatible_types() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.sync(%p: ptr, %expected: i32, %timeout: i64, %count: i32) -> i32 engine(native) {\nbb0(%p: ptr, %expected: i32, %timeout: i64, %count: i32):\n  %wait = atomic.wait %p, %expected, %timeout\n  %woken = atomic.notify %p, %count\n  %result = add %wait, %woken\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("wait/notify text parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("wait/notify CODE assembles"))
            .expect("wait/notify binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 rejected valid wait/notify operations: {errors:?}"
    );
}

#[test]
fn v2_atomic_rmw_and_cmpxchg_support_scalar_and_tuple_results() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.mutate(%p: ptr, %value: i32, %floatp: ptr, %float: f32) -> i32 engine(native) {\nbb0(%p: ptr, %value: i32, %floatp: ptr, %float: f32):\n  %before = atomic.rmw add [order=acq_rel] %p, %value\n  %floatbefore = atomic.rmw fadd [order=relaxed] %floatp, %float\n  %compare = atomic.cmpxchg [weak] [success_order=acquire] [failure_order=relaxed] %p, %before, %value\n  %prior = tuple.extract %compare, 0\n  return %prior\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("atomic update text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2 text rejects valid atomic update tuple: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("atomic update CODE assembles"))
            .expect("atomic update binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 rejects atomic RMW/CMPXCHG: {errors:?}"
    );
}

#[test]
fn v2_v20_varargs_start_arg_copy_and_end_verify_and_are_unmanaged() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.varargs(%list: ptr, %other: ptr) -> i32 engine(native) abi(c) {\nbb0(%list: ptr, %other: ptr):\n  va.start %list\n  va.copy %other, %list\n  %value = va.arg i32, %list\n  va.end %list\n  va.end %other\n  return %value\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("va_list text parses");
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("va_list CODE encodes"))
            .expect("va_list binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_ne!(analysis.functions[0].reason_mask & (1 << 3), 0);
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2/V20 rejects valid va_list CODE: {errors:?}"
    );
}

#[test]
fn v2_v20_setjmp_returns_twice_is_well_typed_and_unmanaged() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.jump(%buf: ptr) -> i32 engine(native) abi(c) returns_twice {\nbb0(%buf: ptr):\n  %state = setjmp %buf\n  return %state\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("setjmp text parses");
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("setjmp CODE encodes"))
        .expect("setjmp binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_ne!(analysis.functions[0].reason_mask & (1 << 4), 0);
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2/V20 rejects valid setjmp CODE: {errors:?}"
    );
}

#[test]
fn v2_setjmp_without_returns_twice_is_rejected_in_text_and_binary() {
    use tir_format::{BinaryModule, assemble_text_module};
    let source = module(
        "func module @Smoke.jump(%buf: ptr) -> i32 engine(native) abi(c) {\nbb0(%buf: ptr):\n  %state = setjmp %buf\n  return %state\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("non-returns_twice text parses");
    let text_issues = verify_text_module(&parsed, Default::default());
    assert!(
        text_issues
            .iter()
            .any(|issue| issue.code.to_string() == "TIR-VFY-0002"),
        "text verifier accepted setjmp without returns_twice: {text_issues:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("invalid semantic CODE still assembles"),
    )
    .expect("CODE decodes");
    let binary_issues = super::verify_binary_module_records(&binary, u64::MAX);
    assert!(
        binary_issues
            .iter()
            .any(|issue| issue.code.to_string() == "TIR-VFY-0002"),
        "binary verifier accepted setjmp without returns_twice: {binary_issues:?}"
    );
}

#[test]
fn v2_direct_calls_validate_forward_signatures_and_results_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.caller(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %result = call @Smoke.double(%a)\n  call @Smoke.finish(%result)\n  return %result\n}\n\nfunc module @Smoke.double(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  %twice = add %x, %x\n  return %twice\n}\n\nfunc module @Smoke.finish(%x: i32) -> unit engine(native) {\nbb0(%x: i32):\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("direct call source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "direct call source failed V2: {text_errors:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("direct calls encode"))
        .expect("direct calls binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "direct call binary failed V2: {binary_errors:?}"
    );
}

#[test]
fn v19_indirect_calls_preserve_exact_function_types_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.indirect(%f: borrow fn(i32) -> i32, %value: i32) -> i32 engine(native) {\nbb0(%f: borrow fn(i32) -> i32, %value: i32):\n  %result = call.indirect %f : fn(i32) -> i32 (%value)\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("indirect call text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "valid indirect call rejected: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("indirect CODE assembles"))
            .expect("indirect binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "valid indirect CODE rejected: {binary_errors:?}"
    );

    let wrong = module(
        "func module @Smoke.invalid(%f: fn(i32) -> i32, %value: i32) -> i32 engine(native) {\nbb0(%f: fn(i32) -> i32, %value: i32):\n  %result = call.indirect %f : fn(i64) -> i32 (%value)\n  return %result\n}",
        "app",
    );
    let mismatched = parse_text_module(&wrong).expect("mismatched indirect call syntax parses");
    assert!(
        assemble_text_module(&mismatched).is_err(),
        "assembler accepted mismatched function type"
    );
    let issues = verify_text_module(&mismatched, Default::default());
    assert!(
        issues
            .iter()
            .any(|issue| issue.code.to_string() == "TIR-VFY-0019"),
        "text verifier did not report V19: {issues:?}"
    );
}

#[test]
fn v2_v10_tail_call_is_terminating_and_matches_caller_signature() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.start(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  tail.call @Smoke.end(%x)\n}\n\nfunc module @Smoke.end(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  return %x\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("tail call text parses");
    let text_issues = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_issues.is_empty(),
        "V2/V10 rejects tail.call: {text_issues:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("tail CODE encodes"))
        .expect("tail binary decodes");
    let binary_issues = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_issues.is_empty(),
        "V2/V10 binary rejects tail.call: {binary_issues:?}"
    );
}

#[test]
fn v2_v20_longjmp_terminates_and_marks_the_function_unmanaged() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.jump(%buffer: ptr, %value: i32) -> unit engine(native) abi(c) {\nbb0(%buffer: ptr, %value: i32):\n  longjmp %buffer, %value\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("longjmp text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verifier rejected longjmp: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("longjmp CODE assembles"))
            .expect("longjmp binary decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_ne!(analysis.functions[0].reason_mask & (1 << 4), 0);
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier rejected longjmp: {binary_errors:?}"
    );
}

#[test]
fn v2_direct_indirect_and_tail_call_site_attributes_are_binary_valid() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.direct(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  %v = call [cold] [noinline] @Smoke.callee(%x)\n  return %v\n}\n\nfunc module @Smoke.indirect(%f: borrow fn(i32) -> i32, %x: i32) -> i32 engine(native) {\nbb0(%f: borrow fn(i32) -> i32, %x: i32):\n  %v = call.indirect [nounwind] [cold] %f : fn(i32) -> i32 (%x)\n  return %v\n}\n\nfunc module @Smoke.tail(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  tail.call [cold] [noinline] @Smoke.callee(%x)\n}\n\nfunc module @Smoke.callee(%x: i32) -> i32 engine(native) nounwind {\nbb0(%x: i32):\n  return %x\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("attributed call text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verifier rejected call attributes: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("attributed call CODE assembles"),
    )
    .expect("attributed call binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier rejected call attributes: {binary_errors:?}"
    );
}

#[test]
fn v1_v2_switch_int_preserves_multiple_typed_edges_and_selection_merge() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.dispatch(%key: i32, %value: i32) -> i32 engine(native) {\nbb0(%key: i32, %value: i32):\n  switch.int %key, [-1: bb3(%value)], [0: bb2(%value)], default bb1(%value) [merge bb4]\nbb1(%fallback: i32):\n  br bb4(%fallback)\nbb2(%case1: i32):\n  br bb4(%case1)\nbb3(%case0: i32):\n  br bb4(%case0)\nbb4(%result: i32):\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("switch.int source parses");
    let text_issues = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_issues.is_empty(),
        "text verifier rejects valid switch: {text_issues:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("switch.int CODE assembles"))
            .expect("switch.int binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V1/V2 binary rejects valid switch: {errors:?}"
    );
}

#[test]
fn v2_fail_is_only_valid_with_matching_function_error_type() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};
    let valid = module(
        "func module @Smoke.error(%err: i32) -> i64 fails i32 engine(native) {\nbb0(%err: i32):\n  fail %err\n}",
        "app",
    );
    let parsed = parse_text_module(&valid).expect("fails declaration parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verifier rejects valid fail: {text_errors:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("fail CODE assembles"))
        .expect("fail CODE decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier rejects valid fail: {binary_errors:?}"
    );

    let invalid = module(
        "func module @Smoke.error(%err: i32) -> i64 engine(native) {\nbb0(%err: i32):\n  fail %err\n}",
        "app",
    );
    let parsed = parse_text_module(&invalid).expect("untyped fail syntax parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "assembler accepted fail outside fails function"
    );
    let text_errors = verify_text_module(&parsed, Default::default());
    assert!(
        text_errors
            .iter()
            .any(|issue| issue.code.to_string() == "TIR-VFY-0002"),
        "text verifier did not return V2: {text_errors:?}"
    );
}

#[test]
fn v2_block_addr_indirect_branch_is_same_function_and_parameterless() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.indirect_branch() -> unit engine(native) {\nbb0:\n  %address = block.addr bb2\n  br.indirect %address, [bb2, bb1]\nbb1:\n  return\nbb2:\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("indirect branch parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects valid indirect branch: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("indirect branch CODE assembles"),
    )
    .expect("indirect branch binary decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects valid indirect branch: {binary_errors:?}"
    );

    let malicious = module(
        "func module @Smoke.indirect_branch(%untrusted: ptr) -> unit engine(native) {\nbb0(%untrusted: ptr):\n  br.indirect %untrusted, [bb1]\nbb1:\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&malicious).expect("untrusted indirect branch parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "unprovenanced indirect branch assembled"
    );
}
