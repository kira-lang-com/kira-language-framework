//! Inline assembly (section 12.14): `asm "template", "constraints"(args)`
//! reaches LLVM as inline assembly with side effects, and runs natively.

mod support;

use klf_vm::Outcome;
use support::native_run;

const ASM: &str = r#"module @Asm lang "test" target "aarch64-apple-darwin" stage raw

func module @stepped(%x: i64) -> i64 engine(native) {
bb0(%x: i64):
  asm "nop"
  asm "mov x9, $0 // \"quoted\"", "r,~{x9}"(%x)
  %one = const.int i64, 1
  %r = add [wrap] %x, %one
  return %r
}
"#;

#[test]
fn asm_prints_and_parses_back() {
    let module = klf_kir::parse_module(ASM).expect("module parses");
    let printed = module.to_string();
    assert!(printed.contains("asm \"nop\"\n"), "{printed}");
    assert!(
        printed.contains(r#"asm "mov x9, $0 // \"quoted\"", "r,~{x9}"(%x)"#),
        "{printed}"
    );
    let again = klf_kir::parse_module(&printed).expect("printed module parses");
    assert_eq!(again.to_string(), printed);
}

#[test]
fn asm_becomes_llvm_inline_assembly() {
    let module = klf_kir::parse_module(ASM).expect("module parses");
    let ir = klf_llvm::emit_module(&module).expect("module lowers");
    assert!(
        ir.contains(r#"call void asm sideeffect "nop", ""()"#),
        "{ir}"
    );
    assert!(
        ir.contains(r#"call void asm sideeffect "mov x9, $0 // \22quoted\22", "r,~{x9}"(i64 "#),
        "{ir}"
    );
}

#[test]
fn an_asm_result_is_refused() {
    let source = ASM.replace("  asm \"nop\"", "  %bad = asm \"nop\"");
    let module = klf_kir::parse_module(&source).expect("module parses");
    let error = klf_llvm::emit_module(&module).expect_err("an asm result has no type");
    assert!(error.to_string().contains("asm"), "{error}");
}

#[cfg(all(target_arch = "aarch64", target_os = "macos"))]
#[test]
fn asm_runs_natively() {
    assert_eq!(native_run(ASM, "stepped", &[41]), Outcome::Return(Some(42)));
}
