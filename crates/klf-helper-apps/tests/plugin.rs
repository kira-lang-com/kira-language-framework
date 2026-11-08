#![cfg(unix)]

use klf_helper_apps::{
    Artifact, Catalog, Host, InstalledApp, PluginId, Resource, ResourceId, Service,
};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn metadata(module: &str, imported: bool) -> Metadata {
    Metadata {
        module_name: module.into(),
        abis: vec![AbiEntry {
            uuid: [8; 16],
            status: AbiStatus::Current,
        }],
        imports: if imported {
            vec![ImportReq {
                module: "Harmony".into(),
                required: [8; 16],
            }]
        } else {
            vec![]
        },
        sha256: [0; 32],
        unmanaged: vec![],
    }
}

fn installed() -> InstalledApp {
    InstalledApp {
        artifact: Artifact::Native {
            executable: env!("CARGO_BIN_EXE_klf-plugin-guest").into(),
        },
        metadata: metadata("PluginA", true),
    }
}

fn host() -> Host {
    let mut host = Host::default();
    let mut functions: HashMap<String, Service> = HashMap::new();
    functions.insert(
        "add".into(),
        Arc::new(|args| match args {
            [Value::Uint(a), Value::Uint(b)] => Reply::Returned(Value::Uint(a + b)),
            _ => Reply::Refused("invalid parameters".into()),
        }),
    );
    host.export(metadata("Harmony", false), functions).unwrap();
    host
}

#[test]
fn a_marketplace_plugin_calls_only_its_imported_host_api() {
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.plugin-a").unwrap();
    catalog
        .install_trusted_native(id.clone(), installed())
        .unwrap();
    let plugin = host().start(&catalog, &id).unwrap();
    plugin.dispatch_next().unwrap();
    plugin.dispatch_next().unwrap();
    assert!(!plugin.dispatch_next().unwrap());
    assert!(plugin.wait().unwrap().success());
}

#[test]
fn a_marketplace_plugin_calls_host_without_manual_ipc_pumping() {
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.plugin-b").unwrap();
    catalog
        .install_trusted_native(id.clone(), installed())
        .unwrap();
    let plugin = host().start_managed(&catalog, &id).unwrap();
    assert!(plugin.wait().unwrap().success());
}

#[test]
fn an_unknown_or_incompatible_ridl_abi_is_rejected_before_launch() {
    let id = PluginId::new("marketplace.plugin-a").unwrap();
    let mut catalog = Catalog::default();
    let mut wrong_abi = installed();
    wrong_abi.metadata.imports[0].required = [9; 16];
    catalog
        .install_trusted_native(id.clone(), wrong_abi)
        .unwrap();
    let error = host().start(&catalog, &id).err().unwrap();
    assert!(error.to_string().contains("KLF-RT-0200"));
}

#[test]
fn an_uninstalled_id_does_not_start_an_arbitrary_process() {
    let error = host()
        .start(
            &Catalog::default(),
            &PluginId::new("missing.plugin").unwrap(),
        )
        .err()
        .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn untrusted_native_plugins_require_the_host_to_grant_explicit_trust() {
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.untrusted-native").unwrap();
    let denied = catalog.install(id.clone(), installed()).unwrap_err();
    assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(catalog.get(&id).is_none());
    catalog
        .install_trusted_native(id.clone(), installed())
        .unwrap();
    assert!(catalog.get(&id).is_some());
}

#[test]
fn a_managed_plugin_reads_a_resource_transferred_from_the_host_process() {
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.resources").unwrap();
    catalog
        .install_trusted_native(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-resource-guest").into(),
                },
                metadata: metadata("PluginResource", true),
            },
        )
        .unwrap();
    let expected = std::fs::read("Cargo.toml").unwrap();
    let seen = Arc::new(AtomicBool::new(false));
    let checked = Arc::clone(&seen);
    let mut functions: HashMap<String, Service> = HashMap::new();
    functions.insert(
        "resourceBytes".into(),
        Arc::new(move |arguments| {
            if arguments == &[Value::Bytes(expected.clone())] {
                checked.store(true, Ordering::Release);
                Reply::Returned(Value::Void)
            } else {
                Reply::Refused("resource bytes did not match".into())
            }
        }),
    );
    let mut host = Host::default();
    host.export(metadata("Harmony", false), functions).unwrap();
    let plugin = host.start_managed(&catalog, &id).unwrap();
    plugin
        .share(
            ResourceId(7),
            &Resource {
                descriptor: File::open("Cargo.toml").unwrap().into(),
            },
        )
        .unwrap();
    assert!(plugin.wait().unwrap().success());
    assert!(seen.load(Ordering::Acquire));
}

#[test]
fn a_plugin_owned_object_stays_remote_until_the_last_host_proxy_is_dropped() {
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.remote-object").unwrap();
    catalog
        .install_trusted_native(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-object-guest").into(),
                },
                metadata: metadata("PluginObjects", false),
            },
        )
        .unwrap();
    let plugin = host().start(&catalog, &id).unwrap();
    assert!(plugin.dispatch_next().unwrap());
    let reference = plugin.take_object(ResourceId(11)).unwrap();
    assert_eq!(reference.id(), 11);
    let copy = reference.clone();
    drop(reference);
    assert!(plugin.try_wait().unwrap().is_none());
    drop(copy);
    assert!(!plugin.dispatch_next().unwrap());
    assert!(plugin.wait().unwrap().success());
}
