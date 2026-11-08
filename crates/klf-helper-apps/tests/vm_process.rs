#![cfg(unix)]

use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId, Service};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn contract(name: &str, imports_host: bool) -> Metadata {
    Metadata {
        module_name: name.to_owned(),
        abis: vec![AbiEntry {
            uuid: [3; 16],
            status: AbiStatus::Current,
        }],
        imports: if imports_host {
            vec![ImportReq {
                module: "Harmony".into(),
                required: [3; 16],
            }]
        } else {
            vec![]
        },
        sha256: [0; 32],
        unmanaged: vec![],
    }
}

#[test]
fn installed_vm_plugin_calls_host_via_compiler_generated_ridl() {
    let source = r#"
module @MarketplacePlugin lang "kira" target "aarch64-apple-darwin" stage canonical
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
    // Separate Kira units may record the same imported RIDL module and entry.
    // The runtime must coalesce these like the compiler's native adapter path.
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
    let program = klf_vm::lower_module(&module).unwrap();
    let folder = std::env::temp_dir().join(format!("klf-helper-vm-test-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("plugin.klbc");
    let kir = folder.join("plugin.kirb");
    let metadata_file = folder.join("plugin.ridlmeta");
    std::fs::write(&image, klf_vm::encode(&program)).unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();
    std::fs::write(&metadata_file, contract("Plugin", true).encode()).unwrap();

    let mut catalog = Catalog::default();
    catalog
        .set_vm_runtime(env!("CARGO_BIN_EXE_klf-helper-vm").into())
        .unwrap();
    let id = PluginId::new("marketplace.vm-only").unwrap();
    catalog
        .install(
            id.clone(),
            InstalledApp::from_compiled(Artifact::Vm { image, kir }, &metadata_file).unwrap(),
        )
        .unwrap();
    let mut host = Host::default();
    let mut exports: HashMap<String, Service> = HashMap::new();
    exports.insert(
        "add".into(),
        Arc::new(|args| {
            assert_eq!(args, &[Value::Int(-2), Value::Int(44)]);
            Reply::Returned(Value::Int(42))
        }),
    );
    host.export(contract("Harmony", false), exports).unwrap();
    let plugin = host.start(&catalog, &id).unwrap();
    assert!(plugin.dispatch_next().unwrap());
    assert!(!plugin.dispatch_next().unwrap());
    assert!(plugin.wait().unwrap().success());
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn a_compiled_plugin_cannot_claim_host_modules_absent_from_its_ridl_imports() {
    let folder =
        std::env::temp_dir().join(format!("klf-helper-import-check-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let kir = folder.join("plugin.kirb");
    let image = folder.join("plugin.klbc");
    let metadata_file = folder.join("plugin.ridlmeta");
    let module = klf_kir::parse_module(
        "module @Plugin lang \"kira\" target \"aarch64-apple-darwin\" stage canonical",
    )
    .unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();
    std::fs::write(&image, b"fake-image").unwrap();
    std::fs::write(&metadata_file, contract("Plugin", true).encode()).unwrap();
    let artifact = Artifact::Vm { image, kir };
    let error = InstalledApp::from_compiled(artifact, &metadata_file).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("disagree with installed metadata")
    );
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn the_host_refuses_ridl_methods_absent_from_a_compiled_vm_import_map() {
    let module = r#"
module @RestrictedPlugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @harmony_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, 2
  %b = const.int i64, 40
  %result = call.foreign @harmony_add(%a, %b)
  return %result
}

"#;
    let mut module = klf_kir::parse_module(module).unwrap();
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
    let folder =
        std::env::temp_dir().join(format!("klf-ridl-import-allowlist-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("plugin.klbc");
    let kir = folder.join("plugin.kirb");
    std::fs::write(
        &image,
        klf_vm::encode(&klf_vm::lower_module(&module).unwrap()),
    )
    .unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();

    // The trusted test runner deliberately calls Harmony.deleteEverything by
    // name, bypassing generated Kira bindings. It must still be refused.
    let mut catalog = Catalog::default();
    catalog
        .set_vm_runtime(env!("CARGO_BIN_EXE_klf-plugin-guest").into())
        .unwrap();
    let id = PluginId::new("marketplace.restricted-guest").unwrap();
    catalog
        .install(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Vm { image, kir },
                metadata: contract("RestrictedPlugin", true),
            },
        )
        .unwrap();
    let mut host = Host::default();
    let mut functions: HashMap<String, Service> = HashMap::new();
    functions.insert(
        "add".into(),
        Arc::new(|arguments| match arguments {
            [Value::Uint(a), Value::Uint(b)] => Reply::Returned(Value::Uint(a + b)),
            _ => Reply::Refused("invalid arguments".into()),
        }),
    );
    functions.insert(
        "deleteEverything".into(),
        Arc::new(|_| Reply::Returned(Value::Void)),
    );
    host.export(contract("Harmony", false), functions).unwrap();
    let plugin = host.start_managed(&catalog, &id).unwrap();
    assert!(plugin.wait().unwrap().success());
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn an_installed_plugin_cannot_expand_its_ridl_import_map_after_approval() {
    let mut module = klf_kir::parse_module(
        "module @TamperedPlugin lang \"kira\" target \"aarch64-apple-darwin\" stage canonical",
    )
    .unwrap();
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
    let folder = std::env::temp_dir().join(format!("klf-ridl-approved-map-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("plugin.klbc");
    let kir = folder.join("plugin.kirb");
    std::fs::write(&image, b"not-executed").unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();

    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.approved-map").unwrap();
    catalog
        .install(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Vm {
                    image,
                    kir: kir.clone(),
                },
                metadata: contract("TamperedPlugin", true),
            },
        )
        .unwrap();
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.import".into(),
            value: Some("Harmony\tfunction deleteEverything() -> Void\n".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.import.entry".into(),
            value: Some("Harmony\tdeleteEverything\tharmony_delete\tVoid->Void".into()),
        },
    ]);
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();
    let error = Host::default().start(&catalog, &id).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("changed after installation"));
    std::fs::remove_dir_all(folder).unwrap();
}
