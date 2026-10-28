//! The C rules of a target, `CLayout` (sections 8.11 and 26.2): the size and
//! alignment of each C scalar type, whether plain `char` and `wchar_t` are
//! signed, the format of `long double`, and how bit-fields are packed. The
//! tables are those of the C toolchain the target builds with; a Tessera
//! target's are those of the Linux target of its architecture (section 5.8).

use crate::Target;

/// The size and alignment of a C scalar type, in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CScalar {
    pub size: u64,
    pub align: u64,
}

const fn scalar(size: u64, align: u64) -> CScalar {
    CScalar { size, align }
}

/// How consecutive bit-fields share storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitFieldRule {
    /// The System V and Itanium rule: a bit-field takes the next free bits,
    /// unless that would make it cross a boundary of its declared type's
    /// alignment, in which case it starts at the next such boundary.
    Itanium,
    /// Microsoft's rule: consecutive bit-fields share a unit of their
    /// declared type while their declared types have the same size and the
    /// unit has room. Any other field closes the unit.
    Microsoft,
}

/// The format of `long double`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LongDouble {
    /// The same format as `double`.
    Double,
    /// The x87 80-bit extended format, padded to its size.
    X87,
    /// IEEE binary128.
    Quad,
}

/// The C rules of one target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CLayout {
    pub char_signed: bool,
    pub short: CScalar,
    pub int: CScalar,
    pub long: CScalar,
    pub long_long: CScalar,
    /// A data pointer, and `size_t`, `ptrdiff_t`, `intptr_t` and `uintptr_t`.
    pub pointer: CScalar,
    pub wchar: CScalar,
    pub wchar_signed: bool,
    pub float: CScalar,
    pub double: CScalar,
    pub long_double: CScalar,
    pub long_double_format: LongDouble,
    pub bit_fields: BitFieldRule,
}

/// The families of 1.0 targets whose C rules differ.
enum Family {
    Apple,
    Linux,
    Windows { mingw: bool },
    Wasm { emscripten: bool },
}

fn family(triple: &str) -> Option<Family> {
    let fields: Vec<&str> = triple.split('-').collect();
    if fields.first() == Some(&"wasm32") {
        return Some(Family::Wasm {
            emscripten: fields.last() == Some(&"emscripten"),
        });
    }
    if fields.contains(&"windows") {
        return Some(Family::Windows {
            mingw: fields.last() == Some(&"gnu"),
        });
    }
    if fields.contains(&"apple") || fields.iter().any(|f| f.starts_with("darwin")) {
        return Some(Family::Apple);
    }
    let linux = fields
        .iter()
        .any(|f| f.starts_with("linux") || f.starts_with("android") || *f == "tessera");
    linux.then_some(Family::Linux)
}

impl Target {
    /// The C rules of this target, or `None` for a target outside the 1.0
    /// set of section 5.8, whose C rules are not tabled.
    #[must_use]
    pub fn c_layout(&self) -> Option<CLayout> {
        let architecture = self.triple.split('-').next().unwrap_or_default();
        let family = family(&self.triple)?;
        let x86 = match architecture {
            "x86_64" => true,
            "aarch64" | "wasm32" => false,
            _ => return None,
        };
        let (long, wchar, wchar_signed) = match family {
            Family::Windows { .. } => (scalar(4, 4), scalar(2, 2), false),
            Family::Wasm { .. } => (scalar(4, 4), scalar(4, 4), true),
            Family::Apple => (scalar(8, 8), scalar(4, 4), true),
            Family::Linux => (scalar(8, 8), scalar(4, 4), x86),
        };
        let (long_double, long_double_format) = match family {
            Family::Windows { mingw: true } if x86 => (scalar(16, 16), LongDouble::X87),
            Family::Windows { .. } => (scalar(8, 8), LongDouble::Double),
            Family::Apple if !x86 => (scalar(8, 8), LongDouble::Double),
            Family::Wasm { emscripten: true } => (scalar(16, 8), LongDouble::Quad),
            _ if x86 => (scalar(16, 16), LongDouble::X87),
            _ => (scalar(16, 16), LongDouble::Quad),
        };
        let pointer = if matches!(family, Family::Wasm { .. }) {
            scalar(4, 4)
        } else {
            scalar(8, 8)
        };
        Some(CLayout {
            char_signed: x86 || !matches!(family, Family::Linux),
            short: scalar(2, 2),
            int: scalar(4, 4),
            long,
            long_long: scalar(8, 8),
            pointer,
            wchar,
            wchar_signed,
            float: scalar(4, 4),
            double: scalar(8, 8),
            long_double,
            long_double_format,
            bit_fields: if matches!(family, Family::Windows { .. }) {
                BitFieldRule::Microsoft
            } else {
                BitFieldRule::Itanium
            },
        })
    }
}
