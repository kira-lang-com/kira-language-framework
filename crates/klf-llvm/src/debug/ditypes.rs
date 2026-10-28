//! KIR types as DWARF debug types (section 16.3).
//!
//! Every description here is one stock LLDB formats on its own. LLVM lowers
//! opaque pointers, so a record is described by the bytes of its layout and
//! reached through a typedef on a pointer, which is the shape LLDB already
//! prints for C and C++ on a target with opaque pointers.

use super::{DebugInfo, escape};
use klf_kir::Type;

impl DebugInfo<'_> {
    /// The debug type of `ty`. `Unit` and `Never` describe `void`, which only
    /// a subroutine result ever is.
    pub(crate) fn di_type(&mut self, ty: &Type) -> usize {
        if let Some(id) = self.types.get(ty) {
            return *id;
        }
        let id = self.fresh_type(ty);
        self.types.entry(ty.clone()).or_insert(id);
        id
    }

    /// The debug type of what lives at an address a `debug.addr` names. A
    /// handle is a pointer in a register, but the same type behind an address
    /// is the record itself: `#dbg_declare` already dereferences, so naming it
    /// a pointer twice would read one indirection too many.
    pub(crate) fn di_memory(&mut self, ty: &Type) -> usize {
        match place(ty) {
            Some(name) => self.record_layout(name),
            None => self.di_type(ty),
        }
    }

    fn fresh_type(&mut self, ty: &Type) -> usize {
        match ty {
            Type::Integer(1) => self.basic("bool", 1, "DW_ATE_boolean"),
            Type::Integer(bits) if *bits > 1 => {
                let name = format!("i{bits}");
                self.basic(&name, u32::from(*bits), "DW_ATE_signed")
            }
            Type::Integer(bits) => self.basic("i1", u32::from(*bits), "DW_ATE_boolean"),
            Type::Float(bits) => {
                let name = match bits {
                    32 => "float",
                    64 => "double",
                    _ => "fp",
                };
                self.basic(name, u32::from(*bits), "DW_ATE_float")
            }
            Type::Unit | Type::Never => self.basic("void", 0, "DW_ATE_unsigned"),
            Type::Pointer | Type::Function(_) => self.pointer(None),
            Type::Vector(count, element) => self.array(u64::from(*count), element),
            Type::Array(count, element) => self.array(*count, element),
            Type::Tuple(elements) => self.tuple(elements),
            _ => match handle(ty) {
                Some((kind, name)) => {
                    let pointee = self.record_typedef(kind, name);
                    self.pointer(Some(pointee))
                }
                None => self.basic("i8", 8, "DW_ATE_signed"),
            },
        }
    }

    pub(crate) fn basic(&mut self, name: &str, bits: u32, encoding: &str) -> usize {
        self.push(&format!(
            "!DIBasicType(name: \"{name}\", size: {bits}, encoding: {encoding})"
        ))
    }

    fn pointer(&mut self, base: Option<usize>) -> usize {
        let base = base.map_or_else(|| "null".to_owned(), |id| format!("!{id}"));
        self.push(&format!(
            "!DIDerivedType(tag: DW_TAG_pointer_type, baseType: {base}, size: {})",
            self.pointer_size
        ))
    }

    fn array(&mut self, count: u64, element: &Type) -> usize {
        let layout = klf_layout::layout_of(element, &self.module.types);
        let element = self.di_type(element);
        let element = self.push(&format!(
            "!DIDerivedType(tag: DW_TAG_member, name: \"0\", baseType: !{element}, size: {})",
            bits(layout.size)
        ));
        let elements = self.list(&[
            format!("!DISubrange(count: {count})"),
            format!("!{element}"),
        ]);
        self.distinct(&format!(
            "!DICompositeType(tag: DW_TAG_array_type, elements: !{elements})"
        ))
    }

    fn tuple(&mut self, members: &[Type]) -> usize {
        let declared: Vec<String> = members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let ty = self.di_type(member);
                let size = klf_layout::layout_of(member, &self.module.types).size;
                format!(
                    "!DIDerivedType(tag: DW_TAG_member, name: \"{index}\", baseType: !{ty}, size: {}, offset: {offset})",
                    bits(size),
                    offset = index as u64 * 8
                )
            })
            .collect();
        let elements = self.list(&declared);
        self.distinct(&format!(
            "!DICompositeType(tag: DW_TAG_structure_type, size: {}, elements: !{elements})",
            members.len() as u64 * 8
        ))
    }

    /// The typedef a record is reached through, named after the KIR spelling
    /// of the type so that LLDB prints `ref Point *` for a handle.
    fn record_typedef(&mut self, kind: &str, name: &str) -> usize {
        let key = format!("{kind}{name}");
        if let Some(id) = self.typedefs.get(&key) {
            return *id;
        }
        let id = self.reserve();
        self.typedefs.insert(key.clone(), id);
        let layout = self.record_layout(name);
        self.fill(
            id,
            format!("!DIDerivedType(tag: DW_TAG_typedef, name: \"{key}\", baseType: !{layout})"),
        );
        id
    }

    /// The `DW_TAG_structure_type` carrying a record's bytes: its members at
    /// the offsets the layout gives them, or, for an enum declaration, its
    /// tag as a zero-length member plus a variant part naming every case.
    pub(crate) fn record_layout(&mut self, name: &str) -> usize {
        if let Some(id) = self.layouts.get(name) {
            return *id;
        }
        let Some(declaration) = self
            .module
            .types
            .iter()
            .find(|one| one.name.as_str() == name)
        else {
            return self.opaque(name);
        };
        let id = self.reserve();
        self.layouts.insert(name.to_owned(), id);
        let nominal = Type::Nominal(declaration.name.clone(), Vec::new());
        let layout = klf_layout::layout_of(&nominal, &self.module.types);
        let text = if declaration.variants.is_empty() {
            self.structure(name, layout.size, declaration)
        } else {
            self.variant(name, layout.size, declaration)
        };
        self.fill(id, text);
        id
    }

    fn opaque(&mut self, name: &str) -> usize {
        let id = self.distinct(&format!(
            "!DICompositeType(tag: DW_TAG_structure_type, name: \"{name}\", size: {})",
            bits(8)
        ));
        self.layouts.insert(name.to_owned(), id);
        id
    }

    fn structure(
        &mut self,
        name: &str,
        size: u64,
        declaration: &klf_kir::TypeDeclaration,
    ) -> String {
        let offsets = klf_layout::field_offsets(name, &self.module.types);
        let declared: Vec<String> = declaration
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let base = self.di_type(&field.ty);
                let offset = field.offset.unwrap_or_else(|| offsets.get(index).copied().unwrap_or(0));
                let size = klf_layout::layout_of(&field.ty, &self.module.types).size;
                format!(
                    "!DIDerivedType(tag: DW_TAG_member, name: \"{}\", baseType: !{base}, size: {}, offset: {offset})",
                    escape(field.name.as_str()),
                    bits(size)
                )
            })
            .collect();
        let elements = self.list(&declared);
        format!(
            "!DICompositeType(tag: DW_TAG_structure_type, name: \"{name}\", size: {}, elements: !{elements})",
            bits(size)
        )
    }

    fn variant(&mut self, name: &str, size: u64, declaration: &klf_kir::TypeDeclaration) -> String {
        let tag_bits = klf_layout::enum_declaration_tag_bits(declaration);
        let tag = self.di_type(&Type::Integer(tag_bits));
        let cases: Vec<String> = declaration
            .variants
            .iter()
            .enumerate()
            .map(|(index, variant)| {
                let (payload, payload_size) = match variant.payload.as_slice() {
                    [] => (0, 0),
                    [one] => (
                        self.di_type(one),
                        klf_layout::layout_of(one, &self.module.types).size,
                    ),
                    many => {
                        let tuple = Type::Tuple(many.to_vec());
                        (
                            self.di_type(&tuple),
                            klf_layout::layout_of(&tuple, &self.module.types).size,
                        )
                    }
                };
                let base = match payload {
                    0 => "null".to_owned(),
                    payload => format!("!{payload}"),
                };
                format!(
                    "!DIDerivedType(tag: DW_TAG_member, name: \"{}\", baseType: {base}, size: {}, extraData: i32 {index})",
                    escape(variant.name.as_str()),
                    bits(payload_size)
                )
            })
            .collect();
        let cases = self.list(&cases);
        let tag_size = u64::from(tag_bits) / 8;
        let discriminator = self.push(&format!(
            "!DIDerivedType(tag: DW_TAG_member, baseType: !{tag}, size: {}, offset: 0, flags: DIFlagArtificial)",
            bits(tag_size)
        ));
        let part = self.distinct(&format!(
            "!DICompositeType(tag: DW_TAG_variant_part, size: {}, elements: !{cases}, discriminator: !{discriminator})",
            bits(size)
        ));
        let elements = self.list(&[format!("!{part}")]);
        format!(
            "!DICompositeType(tag: DW_TAG_structure_type, name: \"{name}\", size: {}, elements: !{elements})",
            bits(size)
        )
    }
}

/// The KIR spelling of a type reached through a handle, and the record it
/// names: the empty prefix for an address of a record, `ref `, `uniq ` and
/// `weak ` for the three ownership policies.
fn handle(ty: &Type) -> Option<(&'static str, &str)> {
    match ty {
        Type::Nominal(name, _) => Some(("", name.as_str())),
        Type::Address(inner) => nominal(inner.as_ref()).map(|name| ("address ", name)),
        Type::Reference(inner) => nominal(inner.as_ref()).map(|name| ("ref ", name)),
        Type::Unique(inner) => nominal(inner.as_ref()).map(|name| ("uniq ", name)),
        Type::Weak(inner) => nominal(inner.as_ref()).map(|name| ("weak ", name)),
        _ => None,
    }
}

/// DWARF states a type's size in bits and a member's offset in bytes.
fn bits(bytes: u64) -> u64 {
    bytes * 8
}

fn nominal(ty: &Type) -> Option<&str> {
    match ty {
        Type::Nominal(name, _) => Some(name.as_str()),
        _ => None,
    }
}

/// The record an address points at: a record itself, or a handle to one.
fn place(ty: &Type) -> Option<&str> {
    match ty {
        Type::Address(inner) => nominal(inner.as_ref()),
        _ => nominal(ty),
    }
}
