#![cfg(unix)]

use klf_helper_apps::services;
use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_vm::{Capabilities, Instance, Outcome};
use std::sync::Arc;

#[test]
fn kira_host_wait_services_a_plugin_call_into_the_same_active_vm() {
    let source = r#"
module @KiraHost lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @plugin_wait(%handle: i64) -> i64 engine(native)
func module @host_add(%a: i64, %b: i64) -> i64 engine(runtime) {
entry(%a: i64, %b: i64):
  %sum = add %a, %b
  return %sum
}
func module @main(%handle: i64) -> i64 engine(runtime) {
entry(%handle: i64):
  %status = call.foreign @plugin_wait(%handle)
  return %status
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

    let mut capabilities = Capabilities::new();
    capabilities.register("plugin_wait", |arguments| {
        let [handle] = arguments else { return u64::MAX };
        services::wait(*handle as i64) as u64
    });
    let vm: Arc<Instance> = Arc::from(
        Instance::load_restricted(klf_vm::lower_module(&module).unwrap(), capabilities).unwrap(),
    );
    assert_eq!(vm.initialize(), Outcome::Return(None));

    let surface = klf_ridl::parse(klf_core::FileId::from_u32(0), "Harmony", contract).unwrap();
    let metadata = Metadata::for_module(
        &surface,
        vec![AbiEntry {
            uuid: [7; 16],
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    let mut host = Host::default();
    host.export_vm(&module, Arc::clone(&vm), metadata).unwrap();

    let mut catalog = Catalog::default();
    catalog
        .install_trusted_native(
            PluginId::new("marketplace.reentrant-host").unwrap(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-plugin-guest").into(),
                },
                metadata: Metadata {
                    module_name: "Plugin".into(),
                    abis: vec![AbiEntry {
                        uuid: [9; 16],
                        status: AbiStatus::Current,
                    }],
                    imports: vec![ImportReq {
                        module: "Harmony".into(),
                        required: [7; 16],
                    }],
                    sha256: [0; 32],
                    unmanaged: vec![],
                },
            },
        )
        .unwrap();
    services::initialize(host, catalog).unwrap();

    let plugin = services::start("marketplace.reentrant-host");
    assert!(plugin > 0);
    let main = vm.function_index("main").unwrap();
    assert_eq!(vm.call(main, &[plugin as u64]), Outcome::Return(Some(0)));
    assert_eq!(services::poll(plugin), services::INVALID_HANDLE);
}
