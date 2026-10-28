//! Address-only lowering (pass 5): value copy/move/destroy on an address-only
//! type become copy.addr/move.addr/destroy.addr on stack slots.

use klf_kir::parse_module;
use klf_kir_passes::canonicalize;
use klf_kir_verify::verify_module;

const ADDRESS_ONLY: &str = r#"module @AO lang "test" target "aarch64-apple-darwin" stage raw

type %Big = struct ridl { a: i64 }
policy { copy: implicit(@copyfn), destroy: fn(@dropfn), relocate: fn(@reloc), exclusivity: static, pairing: none }

func private @use(%x: %Big) -> i64 engine(runtime) {
bb0(%x: %Big):
  %c = copy %x
  destroy %c
  destroy %x
  %z = const.int i64, 0
  return %z
}
"#;

#[test]
fn value_ops_lower_to_address_ops() {
    let raw = parse_module(ADDRESS_ONLY).unwrap();
    let (canonical, diagnostics) = canonicalize(&raw);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let text = canonical.to_string();
    assert!(text.contains("copy.addr"), "expected copy.addr:\n{text}");
    assert!(
        text.contains("destroy.addr"),
        "expected destroy.addr:\n{text}"
    );
    assert!(
        !text.contains("  copy %"),
        "no plain copy of an address-only value should remain:\n{text}"
    );

    assert!(
        verify_module(&canonical).is_empty(),
        "{:?}",
        verify_module(&canonical)
    );
}
