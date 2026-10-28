//! Layer-1 size/alignment/offset computation for KIR types (section 21 layout,
//! the section-32.1 split from `klf-ridl`). The backends and the frontend need
//! field offsets to lower aggregates; this is the one shared source, on a
//! 64-bit target.

use klf_kir::{Type, TypeDeclaration, TypeKind};

mod trace;
pub use trace::trace_map;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// The size and alignment of a type, in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

impl Layout {
    #[must_use]
    pub const fn new(size: u64, align: u64) -> Self {
        Self { size, align }
    }
}

const POINTER: Layout = Layout::new(8, 8);

fn round_up(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}

fn declaration<'a>(name: &str, declarations: &'a [TypeDeclaration]) -> Option<&'a TypeDeclaration> {
    declarations.iter().find(|d| d.name.as_str() == name)
}

/// A nominal type that holds itself by value, directly or through other
/// types, and so has no finite size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecursiveType {
    pub name: String,
}

/// The layout of `ty`, resolving nominal types against `declarations`. An
/// unknown nominal type is treated as a single pointer (an opaque handle).
///
/// A verified module has no type that holds itself by value; for one that
/// does, use [`try_layout_of`], since such a type has no layout to return.
#[must_use]
pub fn layout_of(ty: &Type, declarations: &[TypeDeclaration]) -> Layout {
    try_layout_of(ty, declarations).unwrap_or(POINTER)
}

/// The layout of `ty`.
///
/// # Errors
///
/// Returns the first nominal type found holding itself by value.
pub fn try_layout_of(ty: &Type, declarations: &[TypeDeclaration]) -> Result<Layout, RecursiveType> {
    try_layout_with(ty, &|name| declaration(name, declarations))
}

/// The layout of `ty`, resolving each nominal type through `find`, for a
/// caller that keeps its declarations indexed by name.
///
/// # Errors
///
/// Returns the first nominal type found holding itself by value.
pub fn try_layout_with<'a>(
    ty: &Type,
    find: &dyn Fn(&str) -> Option<&'a TypeDeclaration>,
) -> Result<Layout, RecursiveType> {
    Layouter {
        find,
        open: Vec::new(),
        memo: &mut HashMap::new(),
    }
    .layout(ty)
}

/// The layout of an aggregate over `fields` (declaration order, per-field
/// alignment padding), plus each field's byte offset.
#[must_use]
pub fn aggregate(fields: &[Type], declarations: &[TypeDeclaration]) -> (Layout, Vec<u64>) {
    Layouter {
        find: &|name| declaration(name, declarations),
        open: Vec::new(),
        memo: &mut HashMap::new(),
    }
    .aggregate(fields)
    .unwrap_or((POINTER, vec![0; fields.len()]))
}

/// The byte offsets of a nominal struct's fields, or an empty vector if the type
/// is not a known struct declaration.
#[must_use]
pub fn field_offsets(name: &str, declarations: &[TypeDeclaration]) -> Vec<u64> {
    match declaration(name, declarations) {
        Some(decl) => Layouter {
            find: &|name| self::declaration(name, declarations),
            open: Vec::new(),
            memo: &mut HashMap::new(),
        }
        .fields(decl)
        .map_or_else(|_| vec![0; decl.fields.len()], |(_, offsets)| offsets),
        None => Vec::new(),
    }
}

fn enum_layout(declaration: &TypeDeclaration, declarations: &[TypeDeclaration]) -> (Layout, u64) {
    let tag_size = u64::from(enum_declaration_tag_bits(declaration)) / 8;
    let tag = Layout::new(tag_size, tag_size);
    // The payload area holds any variant: as large as the largest and aligned
    // as the most aligned, which need not be the same variant.
    let payload = declaration
        .variants
        .iter()
        .map(|variant| {
            let fields: Vec<Type> = variant.payload.clone();
            aggregate(&fields, declarations).0
        })
        .fold(Layout::new(0, 1), |area, layout| {
            Layout::new(area.size.max(layout.size), area.align.max(layout.align))
        });
    let payload_offset = round_up(tag.size, payload.align.max(1));
    let align = tag.align.max(payload.align.max(1));
    (
        Layout::new(round_up(payload_offset + payload.size, align), align),
        payload_offset,
    )
}

/// A declaration by its address, which stays put while it is borrowed.
type Key = *const TypeDeclaration;

/// Lays types out while tracking the nominal types being laid out, so one
/// reached again inside itself is reported instead of recursed into forever.
/// Nominal layouts already computed are kept in `memo`, by declaration.
struct Layouter<'a, 'f> {
    find: &'f dyn Fn(&str) -> Option<&'a TypeDeclaration>,
    open: Vec<&'a str>,
    memo: &'f mut HashMap<Key, Layout>,
}

impl<'a> Layouter<'a, '_> {
    fn layout(&mut self, ty: &Type) -> Result<Layout, RecursiveType> {
        Ok(match ty {
            Type::Integer(bits) | Type::Float(bits) => {
                let size = u64::from(bits.div_ceil(8)).max(1);
                Layout::new(size, size)
            }
            Type::Pointer
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address(_)
            | Type::Function(_)
            | Type::Generic(_) => POINTER,
            Type::Unit | Type::Never => Layout::new(0, 1),
            Type::Vector(count, element) => {
                let element = self.layout(element)?;
                Layout::new(element.size * u64::from(*count), element.align.max(1))
            }
            Type::Array(count, element) => {
                let element = self.layout(element)?;
                let stride = round_up(element.size, element.align);
                Layout::new(stride * count, element.align.max(1))
            }
            Type::Tuple(fields) => self.aggregate(fields)?.0,
            Type::Nominal(name, _) => {
                let Some(decl) = (self.find)(name.as_str()) else {
                    return Ok(POINTER);
                };
                let key = std::ptr::from_ref(decl);
                if let Some(layout) = self.memo.get(&key) {
                    return Ok(*layout);
                }
                if self.open.contains(&decl.name.as_str()) {
                    return Err(RecursiveType {
                        name: decl.name.to_string(),
                    });
                }
                self.open.push(decl.name.as_str());
                let layout = if decl.variants.is_empty() {
                    self.fields(decl)?.0
                } else {
                    self.enum_layout(decl)?.0
                };
                self.open.pop();
                self.memo.insert(key, layout);
                layout
            }
        })
    }

    /// A struct's layout and field offsets: the declared ones for an
    /// `explicit` type, whose fields still have to be laid out to find a
    /// type that holds itself.
    fn fields(
        &mut self,
        declaration: &TypeDeclaration,
    ) -> Result<(Layout, Vec<u64>), RecursiveType> {
        let field_types: Vec<Type> = declaration.fields.iter().map(|f| f.ty.clone()).collect();
        let (layout, offsets) = self.aggregate(&field_types)?;
        if declaration.kind != TypeKind::Explicit {
            return Ok((layout, offsets));
        }
        let placed: Vec<u64> = declaration
            .fields
            .iter()
            .map(|field| field.offset.unwrap_or(0))
            .collect();
        let align = declaration.align.map_or(layout.align, u64::from);
        let size = if let Some(size) = declaration.size {
            size
        } else {
            let mut extent = 0;
            for (field, offset) in field_types.iter().zip(&placed) {
                extent = extent.max(offset + self.layout(field)?.size);
            }
            round_up(extent, align.max(1))
        };
        Ok((Layout::new(size, align), placed))
    }

    fn aggregate(&mut self, fields: &[Type]) -> Result<(Layout, Vec<u64>), RecursiveType> {
        let mut offset = 0u64;
        let mut align = 1u64;
        let mut offsets = Vec::with_capacity(fields.len());
        for field in fields {
            let layout = self.layout(field)?;
            offset = round_up(offset, layout.align.max(1));
            offsets.push(offset);
            offset += layout.size;
            align = align.max(layout.align.max(1));
        }
        Ok((Layout::new(round_up(offset, align), align), offsets))
    }

    fn enum_layout(
        &mut self,
        declaration: &'a TypeDeclaration,
    ) -> Result<(Layout, u64), RecursiveType> {
        let tag_size = u64::from(enum_declaration_tag_bits(declaration)) / 8;
        let tag = Layout::new(tag_size, tag_size);
        // As large as the largest variant and aligned as the most aligned.
        let mut payload = Layout::new(0, 1);
        for variant in &declaration.variants {
            let candidate = self.aggregate(&variant.payload)?.0;
            payload = Layout::new(
                payload.size.max(candidate.size),
                payload.align.max(candidate.align),
            );
        }
        let payload_offset = round_up(tag.size, payload.align.max(1));
        let align = tag.align.max(payload.align.max(1));
        Ok((
            Layout::new(round_up(payload_offset + payload.size, align), align),
            payload_offset,
        ))
    }
}

/// The layouts of one module's types, with its declarations indexed by name
/// and every nominal layout and field-offset list computed once.
pub struct Layouts<'a> {
    declarations: HashMap<&'a str, &'a TypeDeclaration>,
    nominal: RefCell<HashMap<Key, Layout>>,
    offsets: RefCell<HashMap<Key, Rc<[u64]>>>,
}

impl<'a> Layouts<'a> {
    #[must_use]
    pub fn new(declarations: &'a [TypeDeclaration]) -> Self {
        let mut index = HashMap::with_capacity(declarations.len());
        for declaration in declarations {
            index
                .entry(declaration.name.as_str())
                .or_insert(declaration);
        }
        Self {
            declarations: index,
            nominal: RefCell::default(),
            offsets: RefCell::default(),
        }
    }

    /// The declaration named `name`.
    #[must_use]
    pub fn declaration(&self, name: &str) -> Option<&'a TypeDeclaration> {
        self.declarations.get(name).copied()
    }

    fn layouter<T>(&self, open: Vec<&'a str>, run: impl FnOnce(&mut Layouter<'a, '_>) -> T) -> T {
        let mut memo = self.nominal.borrow_mut();
        run(&mut Layouter {
            find: &|name| self.declaration(name),
            open,
            memo: &mut memo,
        })
    }

    /// The byte offsets of a tuple's elements, as [`aggregate`].
    #[must_use]
    pub fn tuple_offsets(&self, elements: &[Type]) -> Vec<u64> {
        self.layouter(Vec::new(), |layouter| layouter.aggregate(elements))
            .map_or_else(|_| vec![0; elements.len()], |(_, offsets)| offsets)
    }

    /// As [`layout_of`].
    #[must_use]
    pub fn layout_of(&self, ty: &Type) -> Layout {
        self.layouter(Vec::new(), |layouter| layouter.layout(ty))
            .unwrap_or(POINTER)
    }

    /// As [`field_offsets`].
    #[must_use]
    pub fn field_offsets(&self, name: &str) -> Rc<[u64]> {
        let Some(declaration) = self.declaration(name) else {
            return Rc::from([]);
        };
        if let Some(offsets) = self.offsets.borrow().get(&std::ptr::from_ref(declaration)) {
            return Rc::clone(offsets);
        }
        let offsets: Rc<[u64]> = self
            .layouter(Vec::new(), |layouter| layouter.fields(declaration))
            .map_or_else(
                |_| vec![0; declaration.fields.len()],
                |(_, offsets)| offsets,
            )
            .into();
        self.offsets
            .borrow_mut()
            .insert(std::ptr::from_ref(declaration), Rc::clone(&offsets));
        offsets
    }

    /// As [`enum_payload_offset`].
    #[must_use]
    pub fn enum_payload_offset(&self, name: &str, variant: u32) -> Option<u64> {
        let declaration = self.declaration(name)?;
        declaration.variants.get(variant as usize)?;
        self.layouter(vec![declaration.name.as_str()], |layouter| {
            layouter.enum_layout(declaration)
        })
        .ok()
        .map(|(_, offset)| offset)
    }
}

/// The integer width of an enum tag, following the declaration-count rule.
#[must_use]
pub fn enum_tag_bits(case_count: usize) -> u16 {
    if case_count <= 256 {
        8
    } else if case_count <= 65_536 {
        16
    } else {
        32
    }
}

#[must_use]
pub fn enum_declaration_tag_bits(declaration: &TypeDeclaration) -> u16 {
    if declaration.open {
        32
    } else {
        enum_tag_bits(declaration.variants.len())
    }
}

/// The byte offset of a variant's payload area.
#[must_use]
pub fn enum_payload_offset(
    name: &str,
    variant: u32,
    declarations: &[TypeDeclaration],
) -> Option<u64> {
    let declaration = declaration(name, declarations)?;
    if declaration.variants.is_empty() || declaration.variants.get(variant as usize).is_none() {
        return None;
    }
    Some(enum_layout(declaration, declarations).1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declarations() -> Vec<TypeDeclaration> {
        klf_kir::parse_module(
            "module @layout lang \"test\" target \"x86_64-unknown-linux-gnu\" stage raw\n\
             type %Five = struct ridl { a: i32, b: i32, c: i32, d: i32, e: i32 }\npolicy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: none }\n\
             type %Failure = struct ridl { code: i32, detail: i64 }\npolicy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: none }\n\
             type %Outcome = enum ridl { Ok(%Five), Error(%Failure) }\npolicy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: none }\n",
        )
        .expect("the module parses")
        .types
    }

    // The largest variant (20 bytes, aligned on 4) is not the most aligned
    // one (16 bytes, aligned on 8): the payload area must hold the first at
    // the second's alignment, so the last field of `Five` is inside the enum.
    #[test]
    fn enum_payload_holds_largest_variant_at_strictest_alignment() {
        let declarations = declarations();
        let outcome = Type::Nominal("Outcome".into(), Vec::new());
        assert_eq!(layout_of(&outcome, &declarations), Layout::new(32, 8));
        assert_eq!(enum_payload_offset("Outcome", 0, &declarations), Some(8));
        let layouts = Layouts::new(&declarations);
        assert_eq!(layouts.layout_of(&outcome), Layout::new(32, 8));
        assert_eq!(layouts.enum_payload_offset("Outcome", 0), Some(8));
    }
}
