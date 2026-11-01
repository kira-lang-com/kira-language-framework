//! Flags on unmanaged accesses have machine-code meaning for MMIO.
const MODULE: &str = r#"module @RawFlags lang "test" target "aarch64-apple-darwin" stage raw
func module @access(%p: ptr, %x: i64) -> i64 engine(native) {
bb0(%p: ptr, %x: i64):
  raw.store %x, %p align 8 volatile
  %loaded = raw.load i64, %p align 8 volatile
  return %loaded
}
"#;

#[test]
fn volatile_load_and_store_preserve_alignment_in_llvm_ir() {
    let module = klf_kir::parse_module(MODULE).expect("parse flags");
    let ir = klf_llvm::emit_module(&module).expect("lower flagged unmanaged accesses");
    assert!(ir.contains("store volatile i64"), "{ir}");
    assert!(ir.contains("load volatile i64"), "{ir}");
    assert!(ir.contains("align 8"), "{ir}");
}
