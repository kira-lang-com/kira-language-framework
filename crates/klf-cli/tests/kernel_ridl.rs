use std::process::Command;

#[test]
fn kernel_ridl_command_emits_contract_and_entries() {
    let contract = Command::new(env!("CARGO_BIN_EXE_klf"))
        .args(["ridl", "kernel", "--contract"])
        .output()
        .expect("run contract command");
    assert!(
        contract.status.success(),
        "{}",
        String::from_utf8_lossy(&contract.stderr)
    );
    let text = String::from_utf8(contract.stdout).expect("UTF-8 contract");
    let parsed =
        klf_ridl::parse(klf_core::FileId::from_u32(0), "KLFKernel", &text).expect("parse contract");
    assert_eq!(
        parsed,
        klf_driver::kernel_ridl_module().expect("kernel module")
    );

    let wrappers = Command::new(env!("CARGO_BIN_EXE_klf"))
        .args(["ridl", "kernel", "--kcore"])
        .output()
        .expect("run KCore bindings command");
    assert!(wrappers.status.success());
    assert_eq!(
        String::from_utf8(wrappers.stdout).expect("UTF-8 bindings"),
        klf_driver::kcore_service_bindings().expect("KCore bindings")
    );

    let entries = Command::new(env!("CARGO_BIN_EXE_klf"))
        .args([
            "ridl",
            "kernel",
            "--entries",
            "--target",
            klf_target::host_triple(),
        ])
        .output()
        .expect("run entries command");
    assert!(
        entries.status.success(),
        "{}",
        String::from_utf8_lossy(&entries.stderr)
    );
    assert_eq!(
        String::from_utf8(entries.stdout).expect("UTF-8 entries"),
        klf_driver::kernel_ridl_entries(klf_target::host_triple()).expect("kernel entries")
    );
}
