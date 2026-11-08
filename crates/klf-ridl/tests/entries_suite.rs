//! Entries suite (sections 22 and 24.2): the exported `ridlcc` entries and
//! the import adapters reach the methods a module declares.

use std::collections::BTreeMap;

use klf_core::FileId;
use klf_ridl::llvm::{adapters, entries, path, remote_adapters};
use klf_ridl::module::{ExtendBody, ItemKind};
use klf_ridl::parse;

const SOURCE: &str = r"
trait Widget {
    function body(borrow self) -> I64
}

extend Any Widget {
    function padded(borrow self, by: I64) -> Any Widget
}
";

#[test]
fn a_trait_object_method_has_the_path_the_text_spells() {
    let module = parse(FileId::from_u32(0), "KiraUI", SOURCE).unwrap();
    let (target, method) = module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Extend(extend) => match &extend.body {
                ExtendBody::Methods(methods) => Some((extend.target.clone(), methods[0].clone())),
                ExtendBody::Conformance(_) => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(path(Some(&target), &method), "Any Widget.padded");
}

#[test]
fn a_trait_object_method_is_exported_and_imported_under_its_m_symbol() {
    let module = parse(FileId::from_u32(0), "KiraUI", SOURCE).unwrap();
    let named: BTreeMap<String, String> =
        [("Any Widget.padded".to_owned(), "kira_padded".to_owned())].into();
    let defined = |name: &str| name.to_owned();
    let exported = entries(
        &module,
        &named,
        &BTreeMap::new(),
        "x86_64-unknown-linux-gnu",
        &defined,
    )
    .unwrap();
    assert!(
        exported.contains("_RIDL16KiraUI6Widget6paddedM"),
        "{exported}"
    );
    let imported = adapters(&module, &named, "x86_64-unknown-linux-gnu", &defined, true).unwrap();
    assert!(
        imported.contains("_RIDL16KiraUI6Widget6paddedM"),
        "{imported}"
    );
}

#[test]
fn isolated_native_ridl_imports_generate_wire_adapters_not_provider_symbols() {
    let contract = "function add(a: I32, b: I32) -> I32\nfunction ready() -> Bool\n";
    let module = parse(FileId::from_u32(0), "Harmony", contract).unwrap();
    let named: BTreeMap<String, String> = [
        ("add".to_owned(), "harmony_add".to_owned()),
        ("ready".to_owned(), "harmony_ready".to_owned()),
    ]
    .into();
    let ir = remote_adapters(&module, &named, "aarch64-apple-darwin").unwrap();
    assert!(
        ir.contains("define i32 @\"harmony_add\"(i32 %a0, i32 %a1)"),
        "{ir}"
    );
    assert!(ir.contains("sext i32 %a0 to i64"), "{ir}");
    assert!(ir.contains("klfrt_ridl_remote_scalar"), "{ir}");
    assert!(ir.contains("define i8 @\"harmony_ready\"()"), "{ir}");
    assert!(!ir.contains("_RIDL1"), "{ir}");
}

#[test]
fn remote_adapter_rejects_pointer_bearing_imports() {
    let module = parse(
        FileId::from_u32(0),
        "Harmony",
        "function title(text: borrow String) -> String\n",
    )
    .unwrap();
    let named = [("title".to_owned(), "harmony_title".to_owned())].into();
    let error = remote_adapters(&module, &named, "aarch64-apple-darwin").unwrap_err();
    assert!(error.contains("non-scalar"), "{error}");
}

#[test]
fn remote_native_adapters_reject_unsupported_targets() {
    let module = parse(FileId::from_u32(0), "Host", "function ping() -> Void\n").unwrap();
    let named = [("ping".to_owned(), "ping".to_owned())].into();
    assert!(remote_adapters(&module, &named, "x86_64-pc-windows-msvc").is_err());
    assert!(remote_adapters(&module, &named, "wasm32-wasi").is_err());
}

#[test]
fn isolated_float_imports_use_word_bits_at_the_native_boundary() {
    let module = parse(
        FileId::from_u32(0),
        "MathHost",
        "function square(x: F32) -> F32\n",
    )
    .unwrap();
    let named = [("square".to_owned(), "square_native".to_owned())].into();
    let ir = remote_adapters(&module, &named, "x86_64-unknown-linux-gnu").unwrap();
    assert!(ir.contains("bitcast float %a0 to i32"), "{ir}");
    assert!(ir.contains("bitcast i32 %narrow to float"), "{ir}");
}

#[test]
fn remote_scalar_adapter_llvm_is_accepted_by_clang() {
    use std::process::Command;

    let module = parse(
        FileId::from_u32(0),
        "Harmony",
        "function add(a: I32, b: U32) -> I64\nfunction ready() -> Bool\nfunction square(x: F32) -> F32\n",
    )
    .unwrap();
    let named = [
        ("add".to_owned(), "harmony_add".to_owned()),
        ("ready".to_owned(), "harmony_ready".to_owned()),
        ("square".to_owned(), "harmony_square".to_owned()),
    ]
    .into();
    let text = remote_adapters(&module, &named, "aarch64-apple-darwin").unwrap();
    let folder = std::env::temp_dir().join(format!("klf-ridl-remote-llvm-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let input = folder.join("remote.ll");
    let object = folder.join("remote.o");
    std::fs::write(&input, text).unwrap();
    let compiled = Command::new("clang")
        .args(["-Wno-override-module", "-x", "ir", "-c"])
        .arg(&input)
        .arg("-o")
        .arg(&object)
        .output();
    match compiled {
        Ok(output) => assert!(
            output.status.success(),
            "generated remote RIDL LLVM failed to compile: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("could not invoke Clang: {error}"),
    }
    std::fs::remove_dir_all(folder).unwrap();
}

const HANDED: &str = r"
struct Menu {
    let title: String
    let items: [String]
}

function menus(index: I64) -> [Menu]
";

#[test]
fn a_returned_buffer_is_made_unique_before_it_is_handed_over() {
    let module = parse(FileId::from_u32(0), "AppKit", HANDED).unwrap();
    let named: BTreeMap<String, String> = [("menus".to_owned(), "kira_menus".to_owned())].into();
    let defined = |name: &str| name.to_owned();
    let exported = entries(
        &module,
        &named,
        &BTreeMap::from([("Menu".to_owned(), "kira_destroy_menu".to_owned())]),
        "x86_64-unknown-linux-gnu",
        &defined,
    )
    .unwrap();
    assert!(
        exported.contains("call void @\"ridl.unique.[Menu]\"("),
        "{exported}"
    );
    assert!(
        exported.contains("call i1 @\"__klf_ridl_buffer_unique\"("),
        "{exported}"
    );
}
