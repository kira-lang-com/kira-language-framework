use klf_vm::{Capabilities, Import, ImportKind, Instance, Outcome, Program, lower_module};
use std::path::Path;
use std::sync::{Arc, Barrier};

fn program(import: &str) -> Program {
    let source = format!(
        "module @Helper lang \"kira\" target \"aarch64-apple-darwin\" stage canonical\n\
         func private foreign @{import}(%value: i64) -> i64 engine(native)\n\
         func module @main() -> i64 engine(runtime) {{\n\
         entry():\n\
           %answer = const.int i64, 42\n\
           %result = call.foreign @{import}(%answer)\n\
           return %result\n\
         }}\n"
    );
    lower_module(&klf_kir::parse_module(&source).unwrap()).unwrap()
}

#[test]
fn a_restricted_helper_may_call_only_granted_host_services() {
    let program = program("plugin_host_api");
    let mut capabilities = Capabilities::new();
    capabilities.register("plugin_host_api", |arguments| arguments[0] + 1);
    let vm = Instance::load_restricted(program, capabilities).unwrap();
    assert_eq!(vm.initialize(), Outcome::Return(None));
    let main = vm.function_index("main").unwrap();
    assert_eq!(vm.run(main, &[]), Outcome::Return(Some(43)));
}

#[test]
fn ungranted_functions_cannot_fall_through_to_process_symbols() {
    let error = Instance::load_restricted(program("puts"), Capabilities::new())
        .err()
        .unwrap();
    assert!(error.contains("puts"));
}

#[test]
fn a_restricted_helper_cannot_load_native_libraries_or_addresses() {
    for kind in [ImportKind::Address, ImportKind::Library { first: 0 }] {
        let program = Program {
            functions: vec![],
            consts: vec![],
            imports: vec![Import {
                symbol: "libhost.dylib".into(),
                kind,
            }],
            globals: vec![],
            initializers: vec![],
        };
        assert!(Instance::load_restricted(program, Capabilities::new()).is_err());
    }
}

#[test]
fn ordinary_vm_instances_keep_native_compatibility() {
    let program = Program {
        functions: vec![],
        consts: vec![],
        imports: vec![],
        globals: vec![],
        initializers: vec![],
    };
    assert!(Instance::load(program, Path::new("."), Capabilities::new()).is_ok());
}

#[test]
fn a_ridl_callback_does_not_wait_for_a_vm_active_on_another_thread() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let in_callback = Arc::clone(&entered);
    let finished = Arc::clone(&release);
    let mut capabilities = Capabilities::new();
    capabilities.register("block_host", move |arguments| {
        in_callback.wait();
        finished.wait();
        arguments[0]
    });
    let vm: Arc<Instance> =
        Arc::from(Instance::load_restricted(program("block_host"), capabilities).unwrap());
    let main = vm.function_index("main").unwrap();
    let running = Arc::clone(&vm);
    let worker = std::thread::spawn(move || running.call(main, &[]));
    entered.wait();
    assert!(vm.try_call(main, &[]).is_none());
    release.wait();
    assert_eq!(worker.join().unwrap(), Outcome::Return(Some(42)));
}
