use klf_core::FileId;
use klf_driver::{kernel_ridl_entries, kernel_ridl_module};
use klf_ridl::module::ItemKind;
use klf_services::{KERNEL_SERVICE_MANIFEST, KernelServiceManifest};
use std::collections::BTreeMap;

#[test]
fn kernel_contract_and_entries_cover_the_manifest() {
    assert!(kernel_ridl_entries("i686-pc-windows-msvc").is_err());
    let manifest = KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).expect("manifest parses");
    let module = kernel_ridl_module().expect("contract builds");
    let text = klf_ridl::emit(&module);
    let parsed = klf_ridl::parse(FileId::from_u32(0), "KLFKernel", &text).expect("contract parses");
    assert_eq!(parsed, module);
    assert_eq!(module.items.len(), manifest.services().len());

    let ir = kernel_ridl_entries("x86_64-pc-windows-msvc").expect("entries build");
    for (item, service) in module.items.iter().zip(manifest.services()) {
        let ItemKind::Function(function) = &item.kind else {
            panic!("expected function")
        };
        assert_eq!(function.name, service.symbol);
        assert_eq!(function.params.len(), service.signature.parameters.len());
        let symbol = klf_ridl::mangle::function_symbol("KLFKernel", &function.name, None);
        assert!(ir.contains(&format!("@\"{symbol}\"(")), "missing {symbol}");
    }
}

#[test]
fn kernel_entries_compile_for_the_host() {
    let clang = klf_target::llvm_root()
        .expect("RIDL LLVM is provisioned")
        .join("bin")
        .join(klf_target::executable("clang"));
    let directory = std::env::temp_dir().join(format!("klf-kernel-ridl-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create directory");
    let source = directory.join("kernel.ll");
    let object = directory.join("kernel.o");
    std::fs::write(
        &source,
        kernel_ridl_entries(klf_driver::host_triple()).expect("entries"),
    )
    .expect("write IR");
    let output = std::process::Command::new(clang)
        .args(["-c", "-x", "ir"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("run clang");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(object.is_file());
    std::fs::remove_dir_all(directory).expect("remove directory");
}

#[test]
fn kernel_entry_calls_the_service_through_ridl() {
    let clang = klf_target::llvm_root()
        .expect("RIDL LLVM is provisioned")
        .join("bin")
        .join(klf_target::executable("clang"));
    let directory = std::env::temp_dir().join(format!("klf-kernel-call-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create directory");
    let mut module = kernel_ridl_module().expect("contract");
    module.items.retain(|item| matches!(&item.kind, ItemKind::Function(function) if function.name == "klfsvc_math_binary"));
    assert_eq!(module.items.len(), 1);
    let target = klf_driver::host_triple();
    let name = "klfsvc_math_binary".to_owned();
    let callees = BTreeMap::from([(name.clone(), name.clone())]);
    let caller = klf_ridl::bindgen::caller_symbol(&module.name, &name);
    let callers = BTreeMap::from([(name, caller.clone())]);
    let entries =
        klf_ridl::llvm::entries(&module, &callees, &BTreeMap::new(), target, &str::to_owned)
            .expect("entries");
    let adapters = klf_ridl::llvm::adapters(&module, &callers, target, &str::to_owned, false)
        .expect("adapters");
    let entry_path = directory.join("entry.ll");
    let adapter_path = directory.join("adapter.ll");
    let caller_path = directory.join("caller.c");
    let executable = directory.join(klf_target::executable("client"));
    std::fs::write(&entry_path, entries).expect("write entries");
    std::fs::write(&adapter_path, adapters).expect("write adapters");
    std::fs::write(&caller_path, format!("long long klfsvc_math_binary(long long op, long long a, long long b) {{ return op + a * b; }}\nextern long long {caller}(long long, long long, long long);\nint main(void) {{ return {caller}(2, 7, 6) == 44 ? 0 : 1; }}\n")).expect("write caller");
    let output = std::process::Command::new(clang)
        .arg(&entry_path)
        .arg(&adapter_path)
        .arg(&caller_path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("run clang");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = std::process::Command::new(&executable)
        .status()
        .expect("run client");
    assert!(status.success(), "client returned {status}");
    std::fs::remove_dir_all(directory).expect("remove directory");
}
