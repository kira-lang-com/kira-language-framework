//! Entries suite (sections 22 and 24.2): the exported `ridlcc` entries and
//! the import adapters reach the methods a module declares.

use std::collections::BTreeMap;

use klf_core::FileId;
use klf_ridl::llvm::{adapters, entries, path};
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
