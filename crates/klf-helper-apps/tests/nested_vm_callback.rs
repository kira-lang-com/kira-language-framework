#![cfg(unix)]

use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, ManagedApp, PluginId};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use klf_vm::{Capabilities, Instance, Outcome};
use std::sync::{Arc, Mutex, PoisonError};

#[test]
fn helper_to_host_to_helper_ridl_reenters_the_waiting_kira_vm() {
    let source = r#"
module @KiraHost lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @call_helper() -> i64 engine(native)
func module @host_add(%a: i64, %b: i64) -> i64 engine(runtime) {
entry(%a: i64, %b: i64):
  %sum = add %a, %b
  return %sum
}
func module @main() -> i64 engine(runtime) {
entry():
  %result = call.foreign @call_helper()
  return %result
}
"#;
    let mut module = klf_kir::parse_module(source).unwrap();
    let contract = "function add(a: U64, b: U64) -> U64\n";
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.module".into(),
            value: Some("Harmony".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.surface".into(),
            value: Some(contract.into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.entry".into(),
            value: Some("add\thost_add\tU64,U64->U64".into()),
        },
    ]);
    let waiting: Arc<Mutex<Option<ManagedApp>>> = Arc::default();
    let slot = Arc::clone(&waiting);
    let mut capabilities = Capabilities::new();
    capabilities.register("call_helper", move |_| {
        let app = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match app.as_ref().unwrap().call("Helper.compute", vec![]) {
            Ok(Reply::Returned(Value::Uint(value))) => u64::try_from(value).unwrap(),
            _ => u64::MAX,
        }
    });
    let vm: Arc<Instance> = Arc::from(
        Instance::load_restricted(klf_vm::lower_module(&module).unwrap(), capabilities).unwrap(),
    );
    assert_eq!(vm.initialize(), Outcome::Return(None));

    let surface = klf_ridl::parse(klf_core::FileId::from_u32(0), "Harmony", contract).unwrap();
    let metadata = Metadata::for_module(
        &surface,
        vec![AbiEntry {
            uuid: [5; 16],
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    let mut host = Host::default();
    host.export_vm(&module, Arc::clone(&vm), metadata).unwrap();

    let mut catalog = Catalog::default();
    let id = PluginId::new("helper.nested-calls").unwrap();
    catalog
        .install_trusted_native(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-nested-guest").into(),
                },
                metadata: Metadata {
                    module_name: "NestedHelper".into(),
                    abis: vec![AbiEntry {
                        uuid: [6; 16],
                        status: AbiStatus::Current,
                    }],
                    imports: vec![ImportReq {
                        module: "Harmony".into(),
                        required: [5; 16],
                    }],
                    sha256: [0; 32],
                    unmanaged: vec![],
                },
            },
        )
        .unwrap();
    *waiting.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(host.start_managed(&catalog, &id).unwrap());
    let main = vm.function_index("main").unwrap();
    assert_eq!(vm.call(main, &[]), Outcome::Return(Some(42)));
    let helper = waiting
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap();
    assert!(helper.wait().unwrap().success());
}
