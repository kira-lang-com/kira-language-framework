//! The RIDL calling convention, `ridlcc` (section 22): how a value splits into
//! scalar leaves, and whether it travels in registers or by address.
//!
//! The classification here is the frontend's half of section 22.7: a caller
//! or callee built from it hands LLVM's `swiftcc` only scalars and pointers.

use crate::layout::{LayoutEngine, LayoutError};
use crate::types::{Decl, Scalar, Type};

/// The register class a leaf travels in (section 22.3, rule 2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Integer,
    Float,
}

/// One scalar piece of a value: `size` bytes at `offset` from its start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Leaf {
    pub offset: u64,
    pub size: u64,
    pub class: Class,
}

/// The argument registers of each class a target has (section 22.2).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registers {
    pub integer: usize,
    pub float: usize,
}

impl Registers {
    /// The argument registers of `triple`.
    #[must_use]
    pub fn for_target(triple: &str) -> Self {
        if triple.starts_with("wasm32") {
            Self {
                integer: usize::MAX,
                float: usize::MAX,
            }
        } else if triple.starts_with("x86_64") && triple.contains("windows") {
            Self {
                integer: 4,
                float: 4,
            }
        } else if triple.starts_with("x86_64") {
            Self {
                integer: 6,
                float: 8,
            }
        } else {
            Self {
                integer: 8,
                float: 8,
            }
        }
    }
}

/// How a parameter or result travels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Passing {
    /// In registers, one per leaf, in layout order.
    Direct(Vec<Leaf>),
    /// As the address of storage: the caller's for a parameter, the result
    /// slot the caller passes for a result.
    Indirect,
}

/// The leaves of `ty` in layout order (section 22.3, rule 2).
///
/// # Errors
/// Returns [`LayoutError`] for a type without a concrete layout.
pub fn leaves(engine: &LayoutEngine<'_>, ty: &Type) -> Result<Vec<Leaf>, LayoutError> {
    let mut out = Vec::new();
    push_leaves(engine, ty, 0, &mut out)?;
    Ok(out)
}

fn push_leaves(
    engine: &LayoutEngine<'_>,
    ty: &Type,
    base: u64,
    out: &mut Vec<Leaf>,
) -> Result<(), LayoutError> {
    let word = engine.word();
    let integer = |offset, size| Leaf {
        offset,
        size,
        class: Class::Integer,
    };
    match ty {
        Type::Scalar(Scalar::Void) => {}
        Type::Scalar(Scalar::I128 | Scalar::U128) => {
            out.push(integer(base, 8));
            out.push(integer(base + 8, 8));
        }
        Type::Scalar(scalar @ (Scalar::F32 | Scalar::F64)) => out.push(Leaf {
            offset: base,
            size: if *scalar == Scalar::F32 { 4 } else { 8 },
            class: Class::Float,
        }),
        Type::Scalar(_) => out.push(integer(base, engine.layout_of(ty)?.size)),
        Type::String | Type::Array(_) => {
            for index in 0..4 {
                out.push(integer(base + index * word, word));
            }
        }
        Type::Function { .. } | Type::BorrowFunction { .. } | Type::TraitObject(_) => {
            out.push(integer(base, word));
            out.push(integer(base + word, word));
        }
        Type::Tuple(items) => {
            let (_, placements) = engine.aggregate(items)?;
            for (item, placement) in items.iter().zip(placements) {
                push_leaves(engine, item, base + placement.offset, out)?;
            }
        }
        Type::Optional(_) | Type::Result(..) => raw_words(engine, ty, base, out)?,
        Type::Nominal { name, args } => match engine.registry().get(name) {
            Some(Decl::Struct(s)) if s.is_opaque() => out.push(integer(base, word)),
            Some(Decl::Struct(_)) => {
                for placement in engine.struct_fields(name, args)? {
                    let field = field_type(engine, name, &placement.name, args)?;
                    push_leaves(engine, &field, base + placement.offset, out)?;
                }
            }
            Some(Decl::Enum(e)) if e.cases.iter().all(|case| case.payload.is_empty()) => {
                out.push(integer(base, engine.layout_of(ty)?.size));
            }
            Some(Decl::Enum(_)) => raw_words(engine, ty, base, out)?,
            None => return Err(LayoutError::UnknownType(name.clone())),
        },
        Type::Generic(name) => {
            return Err(LayoutError::Unrepresentable(format!(
                "generic parameter `{name}` has no concrete layout"
            )));
        }
    }
    Ok(())
}

/// An enum with a payload is passed as `ceil(size / W)` integer words; the
/// last covers only the bytes the enum has.
fn raw_words(
    engine: &LayoutEngine<'_>,
    ty: &Type,
    base: u64,
    out: &mut Vec<Leaf>,
) -> Result<(), LayoutError> {
    let word = engine.word();
    let size = engine.layout_of(ty)?.size;
    let mut offset = 0;
    while offset < size {
        out.push(Leaf {
            offset: base + offset,
            size: word.min(size - offset),
            class: Class::Integer,
        });
        offset += word;
    }
    Ok(())
}

fn field_type(
    engine: &LayoutEngine<'_>,
    name: &str,
    field: &str,
    args: &[Type],
) -> Result<Type, LayoutError> {
    let Some(Decl::Struct(s)) = engine.registry().get(name) else {
        return Err(LayoutError::UnknownType(name.to_owned()));
    };
    let subst = s
        .generics
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    s.fields
        .iter()
        .flatten()
        .find(|f| f.name == field)
        .map(|f| f.ty.substitute(&subst))
        .ok_or_else(|| LayoutError::UnknownType(format!("{name}.{field}")))
}

/// Whether `ty` is a scalar as rule 4 means it: one that goes on the stack
/// rather than by reference when its class runs out of registers.
#[must_use]
pub const fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Scalar(_))
}

/// Assigns argument registers in order (section 22.3, rules 1 to 4).
#[derive(Clone, Copy, Debug)]
pub struct Allocator {
    integer: usize,
    float: usize,
}

impl Allocator {
    #[must_use]
    pub const fn new(registers: Registers) -> Self {
        Self {
            integer: registers.integer,
            float: registers.float,
        }
    }

    /// How a value of `size` bytes with `leaves` travels, taking the
    /// registers it uses. `scalar` values never go by reference.
    pub fn classify(&mut self, leaves: &[Leaf], size: u64, word: u64, scalar: bool) -> Passing {
        let integers = leaves.iter().filter(|l| l.class == Class::Integer).count();
        let floats = leaves.len() - integers;
        if scalar {
            self.integer = self.integer.saturating_sub(integers);
            self.float = self.float.saturating_sub(floats);
            return Passing::Direct(leaves.to_vec());
        }
        if leaves.len() <= 4 && size <= 4 * word && integers <= self.integer && floats <= self.float
        {
            self.integer -= integers;
            self.float -= floats;
            return Passing::Direct(leaves.to_vec());
        }
        self.integer = self.integer.saturating_sub(1);
        Passing::Indirect
    }

    /// Takes one integer register for an address: a `borrow mut`, an
    /// indirect result, or a generic value.
    pub fn take_address(&mut self) {
        self.integer = self.integer.saturating_sub(1);
    }
}

/// How a result travels (section 22.4): up to four leaves in the direct
/// result registers of their class, anything else through the indirect
/// result address.
#[must_use]
pub fn classify_result(leaves: &[Leaf]) -> Passing {
    let integers = leaves.iter().filter(|l| l.class == Class::Integer).count();
    let floats = leaves.len() - integers;
    if leaves.len() <= 4 && integers <= 4 && floats <= 4 {
        Passing::Direct(leaves.to_vec())
    } else {
        Passing::Indirect
    }
}
