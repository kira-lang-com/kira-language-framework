#![cfg(unix)]

use klf_helper_apps::services;
use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId, Service};
use klf_host_files::handles::{self, Access};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn kira_service_starts_an_installed_app_and_routes_its_ridl_imports() {
    assert_eq!(
        services::start("marketplace.example"),
        services::NOT_CONFIGURED
    );
    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.example").unwrap();
    catalog
        .install_trusted_native(
            id,
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-plugin-guest").into(),
                },
                metadata: Metadata {
                    module_name: "Plugin".into(),
                    abis: vec![AbiEntry {
                        uuid: [7; 16],
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
    catalog
        .install_trusted_native(
            PluginId::new("marketplace.resources").unwrap(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: env!("CARGO_BIN_EXE_klf-resource-guest").into(),
                },
                metadata: Metadata {
                    module_name: "PluginResources".into(),
                    abis: vec![AbiEntry {
                        uuid: [7; 16],
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
    let mut host = Host::default();
    let mut exports: HashMap<String, Service> = HashMap::new();
    exports.insert(
        "add".into(),
        Arc::new(|args| match args {
            [Value::Uint(a), Value::Uint(b)] => Reply::Returned(Value::Uint(a + b)),
            _ => Reply::Refused("invalid parameters".into()),
        }),
    );
    let expected = std::fs::read("Cargo.toml").unwrap();
    let saw_resource = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&saw_resource);
    exports.insert(
        "resourceBytes".into(),
        Arc::new(move |arguments| {
            if arguments == [Value::Bytes(expected.clone())] {
                check.store(true, Ordering::Release);
                Reply::Returned(Value::Void)
            } else {
                Reply::Refused("resource content differed".into())
            }
        }),
    );
    host.export(
        Metadata {
            module_name: "Harmony".into(),
            abis: vec![AbiEntry {
                uuid: [7; 16],
                status: AbiStatus::Current,
            }],
            imports: vec![],
            sha256: [0; 32],
            unmanaged: vec![],
        },
        exports,
    )
    .unwrap();
    services::initialize(host, catalog).unwrap();
    assert_eq!(
        services::start("marketplace.missing"),
        services::NOT_INSTALLED
    );
    assert_eq!(services::start("../escape"), services::NOT_INSTALLED);
    let handle = services::start("marketplace.example");
    assert!(handle > 0);
    let status = services::poll(handle);
    assert!(status == services::STILL_RUNNING || status == 0);
    assert_eq!(services::wait(handle), 0);
    assert_eq!(services::poll(handle), services::INVALID_HANDLE);
    assert_eq!(services::stop(handle), services::INVALID_HANDLE);
    let handle = services::start("marketplace.resources");
    assert!(handle > 0);
    let file = handles::open(b"Cargo.toml", Access::Read);
    assert!(file > 0);
    assert_eq!(
        services::share_file(handle, 0, file),
        services::INVALID_RESOURCE
    );
    assert_eq!(
        services::share_file(handle, 7, 0),
        services::INVALID_RESOURCE
    );
    assert_eq!(services::share_file(handle, 7, file), 1);
    assert_eq!(handles::close(file), 1);
    assert_eq!(services::wait(handle), 0);
    assert!(saw_resource.load(Ordering::Acquire));
}
