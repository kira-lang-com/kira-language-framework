//! Metadata blob (24.3), SHA-256, and linking checks (24.5).

use klf_core::FileId;
use klf_ridl::meta::{
    AbiEntry, AbiStatus, ImportReq, LinkCheck, Metadata, check_import, conflicting_current,
    parse_uuid,
};
use klf_ridl::parse;
use klf_ridl::sha256::{hex, sha256};

#[test]
fn sha256_matches_a_known_vector() {
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

fn uuid(hex: &str) -> [u8; 16] {
    parse_uuid(hex).unwrap()
}

fn geometry() -> Metadata {
    let module = parse(
        FileId::from_u32(0),
        "Geometry",
        "function area(w: I64) -> I64",
    )
    .unwrap();
    Metadata::for_module(
        &module,
        vec![AbiEntry {
            uuid: uuid("6f1c2a9e44d07b31"),
            status: AbiStatus::Current,
        }],
        vec![ImportReq {
            module: "Canvas2D".to_owned(),
            required: uuid("1111"),
        }],
        vec!["_RIDL18Geometry5unsafeF".to_owned()],
    )
}

#[test]
fn metadata_round_trips_through_bytes() {
    let meta = geometry();
    let bytes = meta.encode();
    let back = Metadata::decode(&bytes).unwrap();
    assert_eq!(meta, back);
}

#[test]
fn metadata_hash_covers_canonical_text() {
    let a = parse(FileId::from_u32(0), "M", "function f(x: I64) -> I64").unwrap();
    let b = parse(FileId::from_u32(0), "M", "function f( x : I64 ) -> I64").unwrap();
    let ma = Metadata::for_module(&a, vec![], vec![], vec![]);
    let mb = Metadata::for_module(&b, vec![], vec![], vec![]);
    assert_eq!(ma.sha256, mb.sha256);
}

#[test]
fn decode_rejects_bad_magic() {
    assert!(Metadata::decode(b"NOPE").is_err());
}

#[test]
fn import_of_a_current_uuid_links() {
    let provider = geometry();
    let check = check_import(&provider, &uuid("6f1c2a9e44d07b31"), false);
    assert_eq!(check, LinkCheck::Ok);
}

#[test]
fn import_of_a_missing_uuid_fails() {
    let provider = geometry();
    let check = check_import(&provider, &uuid("dead"), false);
    assert_eq!(check, LinkCheck::Missing);
}

#[test]
fn import_of_a_retired_uuid_warns_and_strict_blocks() {
    let module = parse(FileId::from_u32(0), "Old", "function f(x: I64) -> I64").unwrap();
    let provider = Metadata::for_module(
        &module,
        vec![AbiEntry {
            uuid: uuid("abcd"),
            status: AbiStatus::Retired,
        }],
        vec![],
        vec![],
    );
    assert_eq!(
        check_import(&provider, &uuid("abcd"), false),
        LinkCheck::RetiredWarning { proceed: true }
    );
    assert_eq!(
        check_import(&provider, &uuid("abcd"), true),
        LinkCheck::RetiredWarning { proceed: false }
    );
}

#[test]
fn two_copies_with_different_current_uuids_conflict() {
    let module = parse(FileId::from_u32(0), "M", "function f(x: I64) -> I64").unwrap();
    let a = Metadata::for_module(
        &module,
        vec![AbiEntry {
            uuid: uuid("1111"),
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    let b = Metadata::for_module(
        &module,
        vec![AbiEntry {
            uuid: uuid("2222"),
            status: AbiStatus::Current,
        }],
        vec![],
        vec![],
    );
    assert!(conflicting_current(&a, &b));
    assert!(!conflicting_current(&a, &a));
}
