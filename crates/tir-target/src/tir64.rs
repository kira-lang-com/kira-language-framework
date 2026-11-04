#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CScalar {
    Char,
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Pointer,
    SizeT,
    PtrdiffT,
    IntptrT,
    UintptrT,
    WcharT,
    Char16T,
    Char32T,
    Float16,
    BFloat16,
    Float,
    Double,
    LongDouble,
    Int128,
    Bool,
    MaxAlignT,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CScalarLayout {
    pub size: u16,
    pub align: u16,
    pub signed: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Tir64Model;

pub const TIR64_POINTER_WIDTH: u8 = 64;
pub const TIR64_C_DATA_LAYOUT: &str = "e-m:e-p:64:64-i64:64-i128:128-f128:128-n32:64-S128";
pub const TIR64: Tir64Model = Tir64Model;

impl Tir64Model {
    #[must_use]
    pub const fn c_scalar(self, scalar: CScalar) -> CScalarLayout {
        let (size, signed) = match scalar {
            CScalar::Char => (1, Some(false)),
            CScalar::SignedChar => (1, Some(true)),
            CScalar::UnsignedChar => (1, Some(false)),
            CScalar::Short => (2, Some(true)),
            CScalar::UnsignedShort => (2, Some(false)),
            CScalar::Int => (4, Some(true)),
            CScalar::UnsignedInt => (4, Some(false)),
            CScalar::Long => (8, Some(true)),
            CScalar::UnsignedLong => (8, Some(false)),
            CScalar::LongLong => (8, Some(true)),
            CScalar::UnsignedLongLong => (8, Some(false)),
            CScalar::Pointer => (8, None),
            CScalar::SizeT => (8, Some(false)),
            CScalar::PtrdiffT | CScalar::IntptrT => (8, Some(true)),
            CScalar::UintptrT => (8, Some(false)),
            CScalar::WcharT => (4, Some(false)),
            CScalar::Char16T => (2, Some(false)),
            CScalar::Char32T => (4, Some(false)),
            CScalar::Float16 | CScalar::BFloat16 => (2, None),
            CScalar::Float => (4, None),
            CScalar::Double => (8, None),
            CScalar::LongDouble | CScalar::Int128 | CScalar::MaxAlignT => (16, None),
            CScalar::Bool => (1, None),
        };
        CScalarLayout {
            size,
            align: size,
            signed,
        }
    }
}

#[must_use]
pub const fn bitint_layout(bits: u16) -> Option<CScalarLayout> {
    if bits == 0 || bits > 16_384 {
        return None;
    }
    let bytes = bits.div_ceil(8);
    let mut size = 1;
    while size < bytes {
        size *= 2;
    }
    Some(CScalarLayout {
        size,
        align: if size > 16 { 16 } else { size },
        signed: None,
    })
}
