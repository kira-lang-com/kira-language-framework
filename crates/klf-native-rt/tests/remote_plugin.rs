#![cfg(unix)]

use klf_helper_apps::{Artifact, Catalog, Host, InstalledApp, PluginId, Service};
use klf_ridl::meta::{AbiEntry, AbiStatus, ImportReq, Metadata};
use klf_ridl_wire::{Reply, Value};
use std::collections::HashMap;
use std::sync::Arc;

const CONTRACT: &str =
    "function subtract(a: I32, b: I32) -> I32\nfunction fraction(x: F32) -> F32\n";

#[test]
fn a_native_plugin_uses_its_inherited_ridl_host_connection() {
    if std::env::var_os("KLF_RIDL_CHANNEL_FD").is_some() {
        run_guest();
        return;
    }

    let mut host = Host::default();
    let mut functions: HashMap<String, Service> = HashMap::new();
    functions.insert(
        "subtract".into(),
        Arc::new(|arguments| {
            assert_eq!(arguments, [Value::Int(-2), Value::Int(40)]);
            Reply::Returned(Value::Int(-42))
        }),
    );
    functions.insert(
        "fraction".into(),
        Arc::new(|arguments| {
            assert_eq!(arguments, [Value::Float((1.25_f64).to_bits())]);
            Reply::Returned(Value::Float((3.75_f64).to_bits()))
        }),
    );
    let module = klf_ridl::parse(klf_core::FileId::from_u32(0), "Harmony", CONTRACT).unwrap();
    let metadata = Metadata::for_module(
        &module,
        vec![AbiEntry {
            uuid: [12; 16],
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    host.export(metadata, functions).unwrap();

    let mut catalog = Catalog::default();
    let id = PluginId::new("marketplace.native-ridl").unwrap();
    catalog
        .install_trusted_native(
            id.clone(),
            InstalledApp {
                artifact: Artifact::Native {
                    executable: std::env::current_exe().unwrap(),
                },
                metadata: Metadata {
                    module_name: "NativePlugin".into(),
                    abis: vec![AbiEntry {
                        uuid: [13; 16],
                        status: AbiStatus::Current,
                    }],
                    imports: vec![ImportReq {
                        module: "Harmony".into(),
                        required: [12; 16],
                    }],
                    sha256: [0; 32],
                    unmanaged: vec![],
                },
            },
        )
        .unwrap();
    let guest = host.start_managed(&catalog, &id).unwrap();
    assert!(guest.wait().unwrap().success());
}

fn run_guest() {
    let path = b"Harmony.subtract";
    let arguments = [(-2_i64) as u64, 40];
    let mut result = 0_u64;
    let status = unsafe {
        klf_native_rt::klfrt_ridl_remote_scalar(
            path.as_ptr(),
            path.len() as u64,
            CONTRACT.as_ptr(),
            CONTRACT.len() as u64,
            arguments.as_ptr(),
            arguments.len() as u64,
            &raw mut result,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(result, (-42_i64) as u64);

    let path = b"Harmony.fraction";
    let arguments = [(1.25_f32).to_bits() as u64];
    let status = unsafe {
        klf_native_rt::klfrt_ridl_remote_scalar(
            path.as_ptr(),
            path.len() as u64,
            CONTRACT.as_ptr(),
            CONTRACT.len() as u64,
            arguments.as_ptr(),
            arguments.len() as u64,
            &raw mut result,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(result, (3.75_f32).to_bits() as u64);
}
