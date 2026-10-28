//! Descriptor, witness table and builder suite (section 23), W = 8.

use klf_ridl::descriptor::{Descriptor, Relocation, WitnessTable, builder_descriptor_size, flags};
use klf_ridl::layout::Layout;

const W: u64 = 8;

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}

#[test]
fn trivial_descriptor_has_null_function_pointers() {
    let d = Descriptor::trivial(Layout::new(16, 8));
    assert_eq!(
        d.flags,
        flags::COPYABLE | flags::RELOCATABLE | flags::TRIVIAL_DESTROY
    );
    let (bytes, relocs) = d.emit(W);
    assert_eq!(bytes.len() as u64, Descriptor::image_size(W));
    assert_eq!(le_u32(&bytes, 0), 1); // abi
    assert_eq!(le_u32(&bytes, 4), d.flags);
    assert_eq!(le_u64(&bytes, 8), 16); // size
    assert_eq!(le_u64(&bytes, 16), 8); // align at 8 + W
    assert!(
        relocs.is_empty(),
        "trivial type has no fn-pointer relocations"
    );
}

#[test]
fn descriptor_field_offsets_match_section_23_1() {
    assert_eq!(Descriptor::image_size(W), 8 + 5 * W);
    let d = Descriptor {
        flags: 0,
        size: 1,
        align: 1,
        copy: Some("copy_sym".to_owned()),
        move_fn: Some("move_sym".to_owned()),
        destroy: Some("destroy_sym".to_owned()),
    };
    let (_, relocs) = d.emit(W);
    assert_eq!(
        relocs,
        vec![
            Relocation {
                offset: 8 + 2 * W,
                symbol: "copy_sym".to_owned()
            },
            Relocation {
                offset: 8 + 3 * W,
                symbol: "move_sym".to_owned()
            },
            Relocation {
                offset: 8 + 4 * W,
                symbol: "destroy_sym".to_owned()
            },
        ]
    );
}

#[test]
fn opaque_unique_handle_is_not_copyable() {
    let unique = Descriptor::opaque(W, None, Some("scene_free".to_owned()));
    assert_eq!(unique.flags & flags::COPYABLE, 0);
    assert_ne!(unique.flags & flags::OPAQUE_HANDLE, 0);
    assert_eq!(unique.size, W);

    let shared = Descriptor::opaque(
        W,
        Some("scene_copy".to_owned()),
        Some("scene_free".to_owned()),
    );
    assert_ne!(shared.flags & flags::COPYABLE, 0);
    let (_, relocs) = shared.emit(W);
    // copy and destroy are present; move is null.
    assert_eq!(relocs.len(), 2);
}

#[test]
fn witness_table_lays_out_abi_count_td_then_entries() {
    let wt = WitnessTable {
        type_descriptor: "shape_td".to_owned(),
        entries: vec!["draw".to_owned(), "bounds".to_owned()],
    };
    let (bytes, relocs) = wt.emit(W);
    assert_eq!(le_u32(&bytes, 0), 1); // abi
    assert_eq!(le_u32(&bytes, 4), 2); // count
    assert_eq!(bytes.len() as u64, 8 + 3 * W); // header + td + 2 entries
    assert_eq!(relocs[0].symbol, "shape_td");
    assert_eq!(relocs[0].offset, 8);
    assert_eq!(relocs[1].symbol, "draw");
    assert_eq!(relocs[2].symbol, "bounds");
}

#[test]
fn builder_descriptor_size_includes_argument_area() {
    // A generic type with two argument descriptors in its extension area.
    assert_eq!(
        builder_descriptor_size(W, 2),
        Descriptor::image_size(W) + 2 * W
    );
}

#[test]
fn descriptor_emits_narrow_words_on_wasm32() {
    let d = Descriptor::trivial(Layout::new(4, 4));
    let (bytes, _) = d.emit(4);
    assert_eq!(bytes.len() as u64, Descriptor::image_size(4)); // 8 + 5*4 = 28
    assert_eq!(le_u32(&bytes, 8), 4); // size as a 4-byte word
}
