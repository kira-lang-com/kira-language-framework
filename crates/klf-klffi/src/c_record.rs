//! `@FFI.Struct` and `@FFI.Union` laid out by the target's C rules, and
//! `@FFI.Alias` resolved through them (section 26.2).
//!
//! An alias's `target` is either a C scalar written as C spells it in one
//! word (`char`, `short`, `int`, `long`, `size_t`, `ssize_t`, `ptrdiff_t`,
//! `intptr_t`, `uintptr_t`, `wchar_t`), whose width the target's `CLayout`
//! gives, or another FFI type, which the alias names.

use crate::model::{AnnotationKind, FfiItem, FfiModule, FfiStruct};
use klf_core::Name;
use klf_kir::{CType, Type};
use klf_target::{BitFieldRule, CLayout, CScalar};
use std::collections::HashMap;

/// A record's size and alignment, in bytes, and where each field sits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CRecord {
    pub size: u64,
    pub align: u64,
    pub fields: Vec<CPlacement>,
}

/// Where one field of a record sits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CPlacement {
    pub name: String,
    /// The field's byte offset; for a bit-field, the offset of the unit of
    /// its declared type that holds it.
    pub offset: u64,
    /// For a bit-field, its first bit counted from the start of the record.
    pub bit_offset: Option<u64>,
}

/// A declaration the C rules cannot lay out, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutError {
    pub declaration: String,
    pub message: String,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "`{}`: {}", self.declaration, self.message)
    }
}

impl std::error::Error for LayoutError {}

/// What a field's type is to the C rules: its size and alignment, whether a
/// bit-field may have it, and its KIR type.
#[derive(Clone, Debug)]
pub(crate) struct Shape {
    pub(crate) scalar: CScalar,
    pub(crate) integer: bool,
    pub(crate) kir: Type,
}

/// Lays out the records of one module for one target.
pub(crate) struct Records<'a> {
    c: &'a CLayout,
    declarations: HashMap<&'a str, &'a FfiStruct>,
    done: HashMap<String, CRecord>,
    open: Vec<String>,
}

impl<'a> Records<'a> {
    pub(crate) fn new(module: &'a FfiModule, c: &'a CLayout) -> Self {
        let declarations = module
            .items
            .iter()
            .filter_map(|item| match item {
                FfiItem::Struct(declaration) => Some((declaration.name.as_str(), declaration)),
                _ => None,
            })
            .collect();
        Self {
            c,
            declarations,
            done: HashMap::new(),
            open: Vec::new(),
        }
    }

    /// The layout of the record `name`.
    pub(crate) fn record(&mut self, name: &str) -> Result<CRecord, String> {
        if let Some(record) = self.done.get(name) {
            return Ok(record.clone());
        }
        if self.open.iter().any(|open| open == name) {
            return Err(format!("`{name}` holds itself by value"));
        }
        let declaration = *self
            .declarations
            .get(name)
            .ok_or_else(|| format!("no declaration of `{name}`"))?;
        let union = has(declaration, AnnotationKind::Union);
        if !union && !has(declaration, AnnotationKind::Struct) {
            return Err(format!("`{name}` is not an `@FFI.Struct` or `@FFI.Union`"));
        }
        self.open.push(name.to_owned());
        let mut fields = Vec::with_capacity(declaration.fields.len());
        for field in &declaration.fields {
            let shape = self.shape(&field.ty)?;
            if let Some(bits) = field.bits {
                if !shape.integer {
                    return Err(format!(
                        "bit-field `{}` is not of an integer type",
                        field.name
                    ));
                }
                if bits == 0 || u64::from(bits) > shape.scalar.size * 8 {
                    return Err(format!(
                        "bit-field `{}` is {bits} bits wide, outside 1 to {} for `{}`",
                        field.name,
                        shape.scalar.size * 8,
                        field.ty
                    ));
                }
            }
            fields.push((field.name.clone(), shape.scalar, field.bits));
        }
        self.open.pop();
        let record = if union {
            union_layout(&fields)
        } else {
            struct_layout(&fields, self.c.bit_fields)
        };
        self.done.insert(name.to_owned(), record.clone());
        Ok(record)
    }

    /// What the type named `ty` is to the C rules.
    pub(crate) fn shape(&mut self, ty: &str) -> Result<Shape, String> {
        let c = self.c;
        let integer = |scalar: CScalar| Shape {
            scalar,
            integer: true,
            kir: Type::Integer(u16::try_from(scalar.size * 8).unwrap_or(u16::MAX)),
        };
        let float = |scalar: CScalar| Shape {
            scalar,
            integer: false,
            kir: Type::Float(u16::try_from(scalar.size * 8).unwrap_or(u16::MAX)),
        };
        let pointer = Shape {
            scalar: c.pointer,
            integer: false,
            kir: Type::Pointer,
        };
        Ok(match ty {
            "Bool" | "I8" | "U8" => integer(CScalar { size: 1, align: 1 }),
            "I16" | "U16" => integer(c.short),
            "I32" | "U32" => integer(c.int),
            "I64" | "U64" => integer(c.long_long),
            "ISize" | "USize" => integer(c.pointer),
            "F32" => float(c.float),
            "F64" => float(c.double),
            "CString" | "RawPtr" => pointer,
            _ => {
                let declaration = *self
                    .declarations
                    .get(ty)
                    .ok_or_else(|| format!("unknown type `{ty}`"))?;
                if let Some(alias) = annotation(declaration, AnnotationKind::Alias) {
                    let target = alias
                        .value("target")
                        .ok_or_else(|| format!("`@FFI.Alias` of `{ty}` has no `target`"))?;
                    return match c_scalar(c, target) {
                        Some(scalar) => Ok(integer(scalar)),
                        None if self.open.iter().any(|open| open == ty) => {
                            Err(format!("`{ty}` is an alias of itself"))
                        }
                        None => {
                            self.open.push(ty.to_owned());
                            let shape = self.shape(target);
                            self.open.pop();
                            shape
                        }
                    };
                }
                if let Some(array) = annotation(declaration, AnnotationKind::Array) {
                    let element = array
                        .value("element")
                        .ok_or_else(|| format!("`@FFI.Array` `{ty}` has no `element`"))?;
                    let count: u64 = array
                        .value("count")
                        .and_then(|count| count.parse().ok())
                        .ok_or_else(|| format!("`@FFI.Array` `{ty}` has no integer `count`"))?;
                    let element = self.shape(element)?;
                    return Ok(Shape {
                        scalar: CScalar {
                            size: element.scalar.size * count,
                            align: element.scalar.align,
                        },
                        integer: false,
                        kir: Type::Array(count, Box::new(element.kir)),
                    });
                }
                if has(declaration, AnnotationKind::Pointer)
                    || has(declaration, AnnotationKind::Callback)
                {
                    return Ok(pointer);
                }
                let record = self.record(ty)?;
                Shape {
                    scalar: CScalar {
                        size: record.size,
                        align: record.align,
                    },
                    integer: false,
                    kir: Type::Nominal(Name::new(ty), Vec::new()),
                }
            }
        })
    }

    /// The C type `ty` crosses a foreign signature as: a record by value, a
    /// pointer, or a scalar of its width and signedness. An alias takes the
    /// signedness its C target has under these rules.
    pub(crate) fn c_type(&mut self, ty: &str) -> Result<CType, String> {
        let bits = |size: u64| u16::try_from(size * 8).unwrap_or(u16::MAX);
        Ok(match ty {
            "Void" => CType::Void,
            "Bool" => CType::Bool,
            "I8" | "I16" | "I32" | "I64" | "ISize" => {
                CType::Signed(bits(self.shape(ty)?.scalar.size))
            }
            "U8" | "U16" | "U32" | "U64" | "USize" => {
                CType::Unsigned(bits(self.shape(ty)?.scalar.size))
            }
            "F32" | "F64" => CType::Float(bits(self.shape(ty)?.scalar.size)),
            "CString" | "RawPtr" => CType::Pointer,
            _ => {
                let declaration = *self
                    .declarations
                    .get(ty)
                    .ok_or_else(|| format!("unknown type `{ty}`"))?;
                if let Some(alias) = annotation(declaration, AnnotationKind::Alias) {
                    let target = alias
                        .value("target")
                        .ok_or_else(|| format!("`@FFI.Alias` of `{ty}` has no `target`"))?;
                    let Some(scalar) = c_scalar(self.c, target) else {
                        if self.open.iter().any(|open| open == ty) {
                            return Err(format!("`{ty}` is an alias of itself"));
                        }
                        self.open.push(ty.to_owned());
                        let crossed = self.c_type(target);
                        self.open.pop();
                        return crossed;
                    };
                    let signed = match target {
                        "char" => self.c.char_signed,
                        "wchar_t" => self.c.wchar_signed,
                        "size_t" | "uintptr_t" => false,
                        _ => true,
                    };
                    return Ok(if signed {
                        CType::Signed(bits(scalar.size))
                    } else {
                        CType::Unsigned(bits(scalar.size))
                    });
                }
                if has(declaration, AnnotationKind::Struct)
                    || has(declaration, AnnotationKind::Union)
                {
                    self.record(ty)?;
                    CType::Value(Name::new(ty))
                } else {
                    CType::Pointer
                }
            }
        })
    }
}

fn annotation(declaration: &FfiStruct, kind: AnnotationKind) -> Option<&crate::model::Annotation> {
    declaration.annotations.iter().find(|a| a.kind == kind)
}

fn has(declaration: &FfiStruct, kind: AnnotationKind) -> bool {
    annotation(declaration, kind).is_some()
}

/// The C scalar an alias target spells, when it spells one.
fn c_scalar(c: &CLayout, target: &str) -> Option<CScalar> {
    Some(match target {
        "char" => CScalar { size: 1, align: 1 },
        "short" => c.short,
        "int" => c.int,
        "long" => c.long,
        "size_t" | "ssize_t" | "ptrdiff_t" | "intptr_t" | "uintptr_t" => c.pointer,
        "wchar_t" => c.wchar,
        _ => return None,
    })
}

fn round_up(value: u64, align: u64) -> u64 {
    value.div_ceil(align.max(1)) * align.max(1)
}

fn struct_layout(fields: &[(String, CScalar, Option<u32>)], rule: BitFieldRule) -> CRecord {
    let mut bit = 0u64;
    let mut align = 1u64;
    // The open Microsoft unit: its first bit, its size in bytes, the bits used.
    let mut unit: Option<(u64, u64, u64)> = None;
    let mut placements = Vec::with_capacity(fields.len());
    for (name, scalar, bits) in fields {
        align = align.max(scalar.align);
        let unit_bits = scalar.align * 8;
        let Some(width) = bits.map(u64::from) else {
            unit = None;
            bit = round_up(bit, unit_bits);
            placements.push(CPlacement {
                name: name.clone(),
                offset: bit / 8,
                bit_offset: None,
            });
            bit += scalar.size * 8;
            continue;
        };
        let (first, container) = match rule {
            BitFieldRule::Itanium => {
                let start = bit / unit_bits * unit_bits;
                if bit + width > start + scalar.size * 8 {
                    bit = round_up(bit, unit_bits);
                }
                let first = bit;
                bit += width;
                (first, first / unit_bits * unit_bits)
            }
            BitFieldRule::Microsoft => match unit {
                Some((start, size, used)) if size == scalar.size && used + width <= size * 8 => {
                    unit = Some((start, size, used + width));
                    (start + used, start)
                }
                _ => {
                    let start = round_up(bit, unit_bits);
                    unit = Some((start, scalar.size, width));
                    bit = start + scalar.size * 8;
                    (start, start)
                }
            },
        };
        placements.push(CPlacement {
            name: name.clone(),
            offset: container / 8,
            bit_offset: Some(first),
        });
    }
    CRecord {
        size: round_up(bit.div_ceil(8), align),
        align,
        fields: placements,
    }
}

fn union_layout(fields: &[(String, CScalar, Option<u32>)]) -> CRecord {
    let align = fields.iter().map(|(_, s, _)| s.align).max().unwrap_or(1);
    let size = fields.iter().map(|(_, s, _)| s.size).max().unwrap_or(0);
    CRecord {
        size: round_up(size, align),
        align,
        fields: fields
            .iter()
            .map(|(name, _, bits)| CPlacement {
                name: name.clone(),
                offset: 0,
                bit_offset: bits.map(|_| 0),
            })
            .collect(),
    }
}
