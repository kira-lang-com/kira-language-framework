#![cfg(unix)]

use klf_helper_apps::{ScalarImport, bind_generated_ridl_imports, bind_vm_host_imports};
use klf_ridl_wire::transport::Connection;
use klf_ridl_wire::{Reply, Value};
use klf_vm::{Capabilities, ForeignClass, ForeignSignature, Instance, Outcome, lower_module};
use std::sync::Arc;

#[test]
fn compiled_vm_import_calls_host_ridl_without_native_symbols() {
    let source = r#"
module @Plugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, 2
  %b = const.int i64, 40
  %answer = call.foreign @harmony_add(%a, %b)
  return %answer
}

"#;
    let program = lower_module(&klf_kir::parse_module(source).unwrap()).unwrap();
    let signature = ForeignSignature {
        parameters: vec![ForeignClass::Unsigned(64), ForeignClass::Unsigned(64)],
        result: Some(ForeignClass::Unsigned(64)),
    };
    let (host, plugin) = Connection::pair(14).unwrap();
    let server = std::thread::spawn(move || {
        host.serve_once(|call| {
            assert_eq!(call.function, "Harmony.add");
            assert_eq!(call.arguments, vec![Value::Uint(2), Value::Uint(40)]);
            Reply::Returned(Value::Uint(42))
        })
        .unwrap();
    });
    let mut capabilities = Capabilities::new();
    bind_vm_host_imports(
        &program,
        &[ScalarImport {
            symbol: "harmony_add".into(),
            path: "Harmony.add".into(),
            signature,
        }],
        &Arc::new(plugin),
        &mut capabilities,
    )
    .unwrap();
    let vm = Instance::load_restricted(program, capabilities).unwrap();
    let main = vm.function_index("main").unwrap();
    assert_eq!(vm.initialize(), Outcome::Return(None));
    assert_eq!(vm.run(main, &[]), Outcome::Return(Some(42)));
    server.join().unwrap();
}

#[test]
fn generated_kira_ridl_attributes_bind_without_new_binding_syntax() {
    let source = r#"
module @GeneratedPlugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, -2
  %b = const.int i64, 44
  %result = call.foreign @harmony_add(%a, %b)
  return %result
}
"#;
    let mut module = klf_kir::parse_module(source).unwrap();
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.import".into(),
            value: Some("Harmony\tfunction add(a: I64, b: I64) -> I64\n".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.import.entry".into(),
            value: Some("Harmony\tadd\tharmony_add\tI64,I64->I64".into()),
        },
    ]);
    let program = lower_module(&module).unwrap();
    let (host, plugin) = Connection::pair(25).unwrap();
    let server = std::thread::spawn(move || {
        host.serve_once(|call| {
            assert_eq!(call.function, "Harmony.add");
            assert_eq!(call.arguments, vec![Value::Int(-2), Value::Int(44)]);
            Reply::Returned(Value::Int(42))
        })
        .unwrap();
    });
    let mut capabilities = Capabilities::new();
    bind_generated_ridl_imports(&module, &program, &Arc::new(plugin), &mut capabilities).unwrap();
    let vm = Instance::load_restricted(program, capabilities).unwrap();
    let main = vm.function_index("main").unwrap();
    assert_eq!(vm.initialize(), Outcome::Return(None));
    assert_eq!(vm.run(main, &[]), Outcome::Return(Some(42)));
    server.join().unwrap();
}

#[test]
fn a_generated_pointer_bearing_ridl_contract_is_refused() {
    let source = r#"
module @UnsafePlugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_text(%ptr: ptr) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %zero = const.int i64, 0
  return %zero
}
"#;
    let mut module = klf_kir::parse_module(source).unwrap();
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.import".into(),
            value: Some("Harmony\tfunction text(value: borrow String) -> I64\n".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.import.entry".into(),
            value: Some("Harmony\ttext\tharmony_text\tString->I64".into()),
        },
    ]);
    let program = lower_module(&module).unwrap();
    let (_host, plugin) = Connection::pair(26).unwrap();
    let mut capabilities = Capabilities::new();
    let error =
        bind_generated_ridl_imports(&module, &program, &Arc::new(plugin), &mut capabilities)
            .unwrap_err();
    assert!(error.contains("non-scalar"));
}
