//! RIDL type descriptors, witness tables and builders (section 23), as emitted
//! static data.
//!
//! A descriptor's function-pointer slots (`copy`, `move`, `destroy`) and a
//! witness table's entries are relocations: this module lays out the byte image
//! and reports, for each pointer slot, the symbol the linker must place there.

use crate::layout::Layout;

/// Descriptor flag bits (section 23.1).
pub mod flags {
    pub const COPYABLE: u32 = 1 << 0;
    pub const RELOCATABLE: u32 = 1 << 1;
    pub const TRIVIAL_DESTROY: u32 = 1 << 2;
    pub const OPAQUE_HANDLE: u32 = 1 << 3;
    pub const RUNTIME_BUILT: u32 = 1 << 4;
}

/// A pointer slot in an emitted image that the linker fills with `symbol`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Relocation {
    pub offset: u64,
    pub symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Descriptor {
    pub flags: u32,
    pub size: u64,
    pub align: u64,
    pub copy: Option<String>,
    pub move_fn: Option<String>,
    pub destroy: Option<String>,
}

impl Descriptor {
    /// A descriptor for a trivially-copyable, bitwise-relocatable, trivially
    /// destroyable type (all three function pointers null).
    #[must_use]
    pub fn trivial(layout: Layout) -> Self {
        Self {
            flags: flags::COPYABLE | flags::RELOCATABLE | flags::TRIVIAL_DESTROY,
            size: layout.size,
            align: layout.align,
            copy: None,
            move_fn: None,
            destroy: None,
        }
    }

    /// A descriptor for an opaque handle: size `W`, retained by `copy`, released
    /// by `destroy`; `copy` is `None` for a unique handle (section 21.5).
    #[must_use]
    pub fn opaque(word: u64, copy: Option<String>, destroy: Option<String>) -> Self {
        let mut flags = flags::OPAQUE_HANDLE | flags::RELOCATABLE;
        if copy.is_some() {
            flags |= flags::COPYABLE;
        }
        if destroy.is_none() {
            flags |= flags::TRIVIAL_DESTROY;
        }
        Self {
            flags,
            size: word,
            align: word,
            copy,
            move_fn: None,
            destroy,
        }
    }

    /// The byte size of a descriptor image for pointer width `word`
    /// (base is `8 + 5W`, section 23.1).
    #[must_use]
    pub const fn image_size(word: u64) -> u64 {
        8 + 5 * word
    }

    /// Lay out the descriptor image and its pointer relocations.
    #[must_use]
    pub fn emit(&self, word: u64) -> (Vec<u8>, Vec<Relocation>) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&self.flags.to_le_bytes());
        push_word(&mut bytes, word, self.size);
        push_word(&mut bytes, word, self.align);
        let mut relocs = Vec::new();
        for slot in [&self.copy, &self.move_fn, &self.destroy] {
            let offset = bytes.len() as u64;
            push_word(&mut bytes, word, 0);
            if let Some(symbol) = slot {
                relocs.push(Relocation {
                    offset,
                    symbol: symbol.clone(),
                });
            }
        }
        (bytes, relocs)
    }
}

/// A witness table image (section 23.2): `abi`, `count`, the conforming type's
/// descriptor, then one relocation per entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WitnessTable {
    pub type_descriptor: String,
    pub entries: Vec<String>,
}

impl WitnessTable {
    #[must_use]
    pub fn emit(&self, word: u64) -> (Vec<u8>, Vec<Relocation>) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(
            &(u32::try_from(self.entries.len()).unwrap_or(u32::MAX)).to_le_bytes(),
        );
        let mut relocs = Vec::new();
        let td_offset = bytes.len() as u64;
        push_word(&mut bytes, word, 0);
        relocs.push(Relocation {
            offset: td_offset,
            symbol: self.type_descriptor.clone(),
        });
        for entry in &self.entries {
            let offset = bytes.len() as u64;
            push_word(&mut bytes, word, 0);
            relocs.push(Relocation {
                offset,
                symbol: entry.clone(),
            });
        }
        (bytes, relocs)
    }
}

/// The byte size of a runtime-built descriptor of a generic type with `n_args`
/// argument descriptors in its extension area (section 23.4's `Z` value).
#[must_use]
pub const fn builder_descriptor_size(word: u64, n_args: u64) -> u64 {
    Descriptor::image_size(word) + n_args * word
}

fn push_word(bytes: &mut Vec<u8>, word: u64, value: u64) {
    if word == 4 {
        bytes.extend_from_slice(
            &u32::try_from(value & 0xFFFF_FFFF)
                .unwrap_or(0)
                .to_le_bytes(),
        );
    } else {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
