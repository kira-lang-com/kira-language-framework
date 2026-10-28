//! RIDL evolution suite (section 24.4): freeze, additive change, apply, and the
//! abi-broken check.

use klf_core::FileId;
use klf_ridl::abi::{AbiError, AbiHistory, Freeze, Surface, apply, freeze};
use klf_ridl::parse;

const V1: &str = "aaaaaaaaaaaaaaaa";
const V2: &str = "bbbbbbbbbbbbbbbb";

fn file() -> FileId {
    FileId::from_u32(0)
}

fn surface(src: &str) -> Surface {
    Surface::from_module(&parse(file(), "M", src).unwrap())
}

#[test]
fn first_freeze_records_the_whole_surface() {
    let s = surface("struct Vec3 { let x: F32 }\nfunction area(w: I64) -> I64");
    let history = AbiHistory::new();
    let Freeze::New { record, .. } = freeze(&s, &history, V1).unwrap() else {
        panic!("expected a new record");
    };
    assert_eq!(record.uuid, V1);
    assert_eq!(record.added.len(), 2);
    assert_eq!(record.superseded, [] as [klf_ridl::abi::Shape; 0]);
}

#[test]
fn refreezing_an_unchanged_surface_is_a_no_op() {
    let s = surface("function area(w: I64) -> I64");
    let mut history = AbiHistory::new();
    if let Freeze::New { record, .. } = freeze(&s, &history, V1).unwrap() {
        history.records.push(record);
    }
    assert_eq!(freeze(&s, &history, V2).unwrap(), Freeze::Unchanged);
}

#[test]
fn adding_a_function_is_additive() {
    let mut history = AbiHistory::new();
    let s1 = surface("function area(w: I64) -> I64");
    if let Freeze::New { record, .. } = freeze(&s1, &history, V1).unwrap() {
        history.records.push(record);
    }
    let s2 = surface("function area(w: I64) -> I64\nfunction perimeter(w: I64) -> I64");
    let Freeze::New {
        record,
        superseded_origin,
    } = freeze(&s2, &history, V2).unwrap()
    else {
        panic!("expected a new record");
    };
    assert_eq!(record.added.len(), 1);
    assert_eq!(record.added[0].path, "perimeter");
    assert_eq!(
        superseded_origin,
        [] as [(klf_ridl::abi::Shape, std::string::String); 0]
    );
}

#[test]
fn changing_a_frozen_shape_in_place_is_abi_broken() {
    let mut history = AbiHistory::new();
    let s1 = surface("function area(w: I64) -> I64");
    if let Freeze::New { record, .. } = freeze(&s1, &history, V1).unwrap() {
        history.records.push(record);
    }
    // The signature changed and the old shape is gone.
    let s2 = surface("function area(w: I64, h: I64) -> I64");
    let err = freeze(&s2, &history, V2).unwrap_err();
    assert!(matches!(err, AbiError::Broken { .. }));
}

#[test]
fn keeping_the_old_shape_and_adding_a_new_one_supersedes_it() {
    let mut history = AbiHistory::new();
    let s1 = surface("function area(w: I64) -> I64");
    if let Freeze::New { record, .. } = freeze(&s1, &history, V1).unwrap() {
        history.records.push(record);
    }
    // Old kept, new overload added (same path, different signature).
    let src2 = "function area(w: I64) -> I64\nfunction area(w: I64, h: I64) -> I64";
    let s2 = surface(src2);
    let Freeze::New {
        record,
        superseded_origin,
    } = freeze(&s2, &history, V2).unwrap()
    else {
        panic!("expected a new record");
    };
    assert_eq!(record.added.len(), 1);
    assert_eq!(record.superseded.len(), 1);
    assert_eq!(superseded_origin.len(), 1);
    assert_eq!(superseded_origin[0].1, V1);
}

#[test]
fn apply_stamps_the_superseded_shape_then_refreeze_is_clean() {
    let mut history = AbiHistory::new();
    let s1 = surface("function area(w: I64) -> I64");
    if let Freeze::New { record, .. } = freeze(&s1, &history, V1).unwrap() {
        history.records.push(record);
    }
    let src2 = "function area(w: I64) -> I64\nfunction area(w: I64, h: I64) -> I64";
    let mut module = parse(file(), "M", src2).unwrap();
    let outcome = freeze(&Surface::from_module(&module), &history, V2).unwrap();
    let Freeze::New {
        record,
        superseded_origin,
    } = outcome
    else {
        panic!("expected a new record");
    };
    let stamped = apply(&mut module, &superseded_origin);
    assert_eq!(stamped, 1);
    history.records.push(record);

    // The stamp must not change the shape identity: refreezing is a no-op.
    let s3 = Surface::from_module(&module);
    assert_eq!(
        freeze(&s3, &history, "cccccccccccccccc").unwrap(),
        Freeze::Unchanged
    );

    // The stamped item now carries @Abi(version: V1).
    let stamped_item = module
        .items
        .iter()
        .find(|i| i.annotations.iter().any(|a| a.name == "Abi"))
        .unwrap();
    let arg = &stamped_item.annotations[0].args[0];
    assert_eq!(arg.label.as_deref(), Some("version"));
}
