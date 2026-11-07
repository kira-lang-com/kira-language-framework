use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Varargs tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("varargs text parses");
    let encoded = assemble_text_module(&parsed).expect("varargs CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("varargs CODE decodes");
    let analysis = binary.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("varargs CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("varargs CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn variadic_list_start_arg_copy_and_end_round_trip() {
    let source = module(
        "func module @Varargs.run(%list: ptr, %backup: ptr) -> i32 engine(native) abi(c) {\nbb0(%list: ptr, %backup: ptr):\n  va.start %list\n  va.copy %backup, %list\n  %value = va.arg i32, %list\n  va.end %list\n  va.end %backup\n  return %value\n}",
    );
    let canonical = round_trip(&source);
    for name in ["va.start", "va.arg i32", "va.copy", "va.end"] {
        assert!(canonical.contains(name), "missing {name}");
    }
}

#[test]
fn va_arg_scalar_and_pointer_in_following_block_round_trip() {
    let source = module(
        "func module @Varargs.float(%list: ptr) -> f64 engine(native) abi(c) {\nbb0(%list: ptr):\n  va.start %list\n  br bb1(%list)\nbb1(%state: ptr):\n  %pointer = va.arg ptr, %state\n  %float = va.arg f64, %state\n  va.end %state\n  return %float\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("va.arg ptr"));
    assert!(canonical.contains("va.arg f64"));
}

#[test]
fn setjmp_result_round_trips_with_returns_twice_attribute() {
    let source = module(
        "func module @Varargs.jump(%buffer: ptr) -> i32 engine(native) abi(c) returns_twice {\nbb0(%buffer: ptr):\n  %state = setjmp %buffer\n  return %state\n}",
    );
    assert!(round_trip(&source).contains("setjmp %"));
}

#[test]
fn varargs_reject_bad_list_types_unknown_ssa_and_result_counts() {
    for (params, expression) in [
        ("%list: i64", "va.start %list"),
        ("%list: ptr<device>", "va.end %list"),
        ("%list: ptr", "va.copy %list, %unknown"),
        ("%list: ptr", "%result = va.arg i1, %list"),
        ("%list: ptr", "va.arg i32, %list"),
        ("%list: ptr", "%result = va.start %list"),
        ("%list: ptr", "setjmp %list"),
    ] {
        let source = module(&format!(
            "func module @Varargs.bad({params}) -> unit engine(native) abi(c) {{\nbb0({params}):\n  {expression}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid varargs CODE assembled: {expression}"
            );
        }
    }
}
