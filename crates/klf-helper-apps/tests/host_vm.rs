#![cfg(unix)]

use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_vm::{Capabilities, Instance, Outcome};
use std::sync::Arc;

const SOURCE: &str = r#"
module @KiraHost lang "kira" target "aarch64-apple-darwin" stage canonical
func module @host_add(%a: i64, %b: i64) -> i64 engine(runtime) {
entry(%a: i64, %b: i64):
  %sum = add %a, %b
  return %sum
}
"#;

fn compiled_host() -> (klf_kir::Module, Arc<Instance>, Metadata) {
    let mut module = klf_kir::parse_module(SOURCE).unwrap();
    let ridl_text = "function add(a: U64, b: U64) -> U64\n";
    module.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.module".into(),
            value: Some("Harmony".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.surface".into(),
            value: Some(ridl_text.into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.entry".into(),
            value: Some("add\thost_add\tU64,U64->U64".into()),
        },
    ]);
    let program = klf_vm::lower_module(&module).unwrap();
    let instance: Arc<Instance> =
        Arc::from(Instance::load_restricted(program, Capabilities::new()).unwrap());
    assert_eq!(instance.initialize(), Outcome::Return(None));
    let surface = klf_ridl::parse(klf_core::FileId::from_u32(0), "Harmony", ridl_text).unwrap();
    let metadata = Metadata::for_module(
        &surface,
        vec![AbiEntry {
            uuid: [7; 16],
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    (module, instance, metadata)
}

#[test]
fn plugin_calls_a_compiled_kira_vm_host_through_generated_ridl_entries() {
    let (module, instance, metadata) = compiled_host();
    let mut host = Host::default();
    host.export_vm(&module, instance, metadata).unwrap();

    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.kira-host").unwrap();
    catalog
        .install_trusted_native(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-plugin-guest").into(),
                },
                metadata: Metadata {
                    module_name: "MarketplacePlugin".into(),
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
    let plugin = host.start_managed(&catalog, &id).unwrap();
    assert!(plugin.wait().unwrap().success());
}

#[test]
fn compiled_kira_vm_plugin_calls_compiled_kira_vm_host_over_ridl() {
    let (host_module, instance, metadata) = compiled_host();
    let mut host = Host::default();
    host.export_vm(&host_module, instance, metadata).unwrap();

    let source = r#"
module @KiraPlugin lang "kira" target "aarch64-apple-darwin" stage canonical
func private foreign @host_add(%a: i64, %b: i64) -> i64 engine(native)
func module @main() -> i64 engine(runtime) {
entry():
  %a = const.int i64, 2
  %b = const.int i64, 40
  %result = call.foreign @host_add(%a, %b)
  %expected = const.int i64, 42
  %correct = icmp eq %result, %expected
  cond.br %correct, good(), wrong()
good():
  return %result
wrong():
  %zero = const.int i64, 0
  %failure = udiv %result, %zero
  return %failure
}

"#;
    let mut guest = klf_kir::parse_module(source).unwrap();
    guest.attributes.extend([
        klf_kir::ModuleAttribute {
            name: "ridl.import".into(),
            value: Some("Harmony\tfunction add(a: U64, b: U64) -> U64\n".into()),
        },
        klf_kir::ModuleAttribute {
            name: "ridl.import.entry".into(),
            value: Some("Harmony\tadd\thost_add\tU64,U64->U64".into()),
        },
    ]);
    let bytecode = klf_vm::lower_module(&guest).unwrap();
    let folder = std::env::temp_dir().join(format!("klf-kira-to-kira-ridl-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let image = folder.join("plugin.klbc");
    let kir = folder.join("plugin.kirb");
    let meta = folder.join("plugin.ridlmeta");
    std::fs::write(&image, klf_vm::encode(&bytecode)).unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&guest).unwrap()).unwrap();
    std::fs::write(
        &meta,
        Metadata {
            module_name: "KiraPlugin".into(),
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
        }
        .encode(),
    )
    .unwrap();

    let mut catalog = Catalog::default();
    catalog
        .set_vm_runtime(env!("CARGO_BIN_EXE_klf-helper-vm").into())
        .unwrap();
    let id = PluginId::new("marketplace.kira-vm").unwrap();
    catalog
        .install(
            id.clone(),
            InstalledApp::from_compiled(Artifact::Vm { image, kir }, &meta).unwrap(),
        )
        .unwrap();
    let plugin = host.start_managed(&catalog, &id).unwrap();
    assert!(plugin.wait().unwrap().success());
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_kira_vm_host_refuses_mismatched_ridl_metadata_and_exports() {
    let (module, instance, metadata) = compiled_host();
    let mut host = Host::default();
    let mut forged = metadata.clone();
    forged.sha256 = [0; 32];
    let reason = host
        .export_vm(&module, Arc::clone(&instance), forged)
        .unwrap_err();
    assert!(reason.to_string().contains("does not match"));

    let mut missing = module.clone();
    missing.attributes.retain(|attr| attr.name != "ridl.entry");
    let reason = host
        .export_vm(&missing, Arc::clone(&instance), metadata.clone())
        .unwrap_err();
    assert!(reason.to_string().contains("without generated entries"));

    let mut wrong = module;
    let entry = wrong
        .attributes
        .iter_mut()
        .find(|attr| attr.name == "ridl.entry")
        .unwrap();
    entry.value = Some("add\thost_add\tU64,U64->U64".into());
    let extra = wrong
        .attributes
        .iter_mut()
        .find(|attr| attr.name == "ridl.surface")
        .unwrap();
    extra.value = Some("function add(a: String, b: U64) -> U64\n".into());
    let surface = klf_ridl::parse(
        klf_core::FileId::from_u32(0),
        "Harmony",
        extra.value.as_deref().unwrap(),
    )
    .unwrap();
    let changed = Metadata::for_module(&surface, metadata.abis.clone(), vec![], vec![]);
    let reason = host.export_vm(&wrong, instance, changed).unwrap_err();
    assert!(reason.to_string().contains("non-scalar"));
}

#[test]
fn host_binder_rejects_a_generated_surface_with_the_wrong_scalar_width() {
    let (mut module, instance, metadata) = compiled_host();
    let attr = module
        .attributes
        .iter_mut()
        .find(|a| a.name == "ridl.surface")
        .unwrap();
    attr.value = Some("function add(a: U32, b: U64) -> U64\n".into());
    let parsed = klf_ridl::parse(
        klf_core::FileId::from_u32(0),
        "Harmony",
        attr.value.as_deref().unwrap(),
    )
    .unwrap();
    let changed = Metadata::for_module(&parsed, metadata.abis, vec![], vec![]);
    let reason = Host::default()
        .export_vm(&module, instance, changed)
        .unwrap_err();
    assert!(reason.to_string().contains("KIR signature"));
}

#[test]
fn a_kira_host_bundle_registers_its_generated_ridl_exports_from_compiled_files() {
    let (module, _source_instance, metadata) = compiled_host();
    let directory = std::env::temp_dir().join(format!("klf-host-bundle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let image = directory.join("host.klbc");
    let kir = directory.join("host.kirb");
    let ridl = directory.join("host.ridlmeta");
    std::fs::write(
        &image,
        klf_vm::encode(&klf_vm::lower_module(&module).unwrap()),
    )
    .unwrap();
    std::fs::write(&kir, klf_kir::kirb::encode(&module).unwrap()).unwrap();
    std::fs::write(&ridl, metadata.encode()).unwrap();

    let mut host = Host::default();
    let instance = host
        .export_compiled_vm(&image, &kir, &ridl, Capabilities::new())
        .unwrap();
    let function = instance.function_index("host_add").unwrap();
    assert_eq!(instance.call(function, &[2, 40]), Outcome::Return(Some(42)));

    let mut catalog = Catalog::default();
    let plugin_id = PluginId::new("marketplace.exported-kira-host").unwrap();
    catalog
        .install_trusted_native(
            plugin_id.clone(),
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
    let plugin = host.start_managed(&catalog, &plugin_id).unwrap();
    assert!(plugin.wait().unwrap().success());
    std::fs::remove_dir_all(directory).unwrap();
}
