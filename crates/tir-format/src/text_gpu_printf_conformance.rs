use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str, target: &str) -> String {
    format!("module @GpuPrintf tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n").replace("_TARGET_", target)
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("GPU printf source parses");
    let bytes = assemble_text_module(&parsed).expect("GPU printf CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("GPU printf TIRB decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("GPU printf disassembles");
    let rebuilt = assemble_text_module(
        &parse_text_module(&canonical).expect("canonical GPU printf source parses"),
    )
    .expect("GPU printf CODE reassembles");
    assert_eq!(bytes, rebuilt, "GPU printf binary round-trip changed bytes");
    canonical
}

#[test]
fn gpu_printf_no_arguments_and_empty_format_round_trip() {
    let source = module(
        "func module @GpuPrintf.say() -> unit _TARGET_ {\nbb0:\n  gpu.printf \"Hello!\"\n  gpu.printf \"\"\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.printf \"Hello!\"\n"));
    assert!(canonical.contains("gpu.printf \"\"\n"));
}

#[test]
fn gpu_printf_formats_multiple_ssa_values_and_preserves_escapes() {
    let source = module(
        "func module @GpuPrintf.multi() -> unit _TARGET_ {\nbb0:\n  %a = const.int i32, 7\n  %b = const.float f32, 1.5\n  gpu.printf \"value=%d float=%f\\n\", %a, %b\n  gpu.printf \"say \\\"hello\\\" \\\\ end\", %a\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("gpu.printf \"value=%d float=%f\\n\", "));
    assert!(canonical.contains("gpu.printf \"say \\\"hello\\\" \\\\ end\", "));
}

#[test]
fn gpu_printf_repeated_strings_across_functions_and_blocks_round_trip() {
    let source = module(
        "func module @GpuPrintf.first() -> i32 _TARGET_ {\nbb0:\n  %value = gpu.spec_const 10, i32, -5\n  gpu.printf \"value=%d\", %value\n  br bb1(%value)\nbb1(%argument: i32):\n  gpu.printf \"value=%d\", %argument\n  return %argument\n}\n\nfunc module @GpuPrintf.second() -> unit _TARGET_ {\nbb0:\n  %value = gpu.builtin lane_index : i32\n  gpu.printf \"value=%d\", %value\n  return\n}\n",
        "target(gpu)",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("gpu.printf \"value=%d\"").count(), 3);
}

#[test]
fn gpu_printf_rejects_non_gpu_target_bad_format_and_non_ssa_arguments() {
    for (statement, target) in [
        ("gpu.printf \"text\"", "engine(native)"),
        ("gpu.printf", "target(gpu)"),
        ("gpu.printf 12", "target(gpu)"),
        ("%result = gpu.printf \"text\"", "target(gpu)"),
        ("gpu.printf \"text\", 7", "target(gpu)"),
        ("gpu.printf \"text\", value", "target(gpu)"),
        ("gpu.printf \"text\", %unknown", "target(gpu)"),
        ("gpu.printf \"text\", %unknown,", "target(gpu)"),
        ("gpu.printf \"text\", %unknown,, %unknown", "target(gpu)"),
        ("gpu.printf \"bad\\q\"", "target(gpu)"),
    ] {
        let source = module(
            &format!(
                "func module @GpuPrintf.invalid() -> unit _TARGET_ {{\nbb0:\n  {statement}\n  return\n}}"
            ),
            target,
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid gpu.printf accepted: {statement} ({target})"
            );
        }
    }
}
