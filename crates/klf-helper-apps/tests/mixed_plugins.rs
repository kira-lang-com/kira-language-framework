#![cfg(unix)]

use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId, Service};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn metadata(name: &str, imports_host: bool) -> Metadata {
    Metadata {
        module_name: name.into(),
        abis: vec![AbiEntry {
            uuid: [4; 16],
            status: AbiStatus::Current,
        }],
        imports: if imports_host {
            vec![ImportReq {
                module: "Harmony".into(),
                required: [4; 16],
            }]
        } else {
            vec![]
        },
        sha256: [0; 32],
        unmanaged: vec![],
    }
}

#[test]
fn one_host_runs_vm_and_native_plugins_with_separate_ridl_connections() {
    let source = r#"
module @VmPlugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, 2
  %b = const.int i64, 40
  %answer = call.foreign @harmony_add(%a, %b)
  return %answer
}
"#;
    let mut module = klf_kir::parse_module(source).unwrap();
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.import".into(),
            value: Some("Harmony\tfunction add(a: U64, b: U64) -> U64\n".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.import.entry".into(),
            value: Some("Harmony\tadd\tharmony_add\tU64,U64->U64".into()),
        },
    ]);
    let folder = std::env::temp_dir().join(format!("klf-mixed-plugins-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("plugin.klbc");
    let kir = folder.join("plugin.kirb");
    let meta = folder.join("plugin.ridlmeta");
    std::fs::write(
        &image,
        klf_vm::encode(&klf_vm::lower_module(&module).unwrap()),
    )
    .unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();
    std::fs::write(&meta, metadata("VmPlugin", true).encode()).unwrap();

    let mut catalog = Catalog::default();
    catalog
        .set_vm_runtime(env!("CARGO_BIN_EXE_klf-helper-vm").into())
        .unwrap();
    let vm_id = PluginId::new("marketplace.vm").unwrap();
    let native_id = PluginId::new("marketplace.native").unwrap();
    catalog
        .install(
            vm_id.clone(),
            InstalledApp::from_compiled(Artifact::Vm { image, kir }, &meta).unwrap(),
        )
        .unwrap();
    catalog
        .install_trusted_native(
            native_id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-plugin-guest").into(),
                },
                metadata: metadata("NativePlugin", true),
            },
        )
        .unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&calls);
    let mut exports: HashMap<String, Service> = HashMap::new();
    exports.insert(
        "add".into(),
        Arc::new(move |arguments| {
            assert_eq!(arguments, [Value::Uint(2), Value::Uint(40)]);
            called.fetch_add(1, Ordering::Relaxed);
            Reply::Returned(Value::Uint(42))
        }),
    );
    let mut host = Host::default();
    host.export(metadata("Harmony", false), exports).unwrap();

    let vm = host.start_managed(&catalog, &vm_id).unwrap();
    let native = host.start_managed(&catalog, &native_id).unwrap();
    assert!(vm.wait().unwrap().success());
    assert!(native.wait().unwrap().success());
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    std::fs::remove_dir_all(folder).unwrap();
}
