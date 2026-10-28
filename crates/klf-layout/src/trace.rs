//! The trace maps of the debug cycle detector (section 18.6): where the
//! strong references sit in a counted object's payload. The encoding is the
//! one `klf-rt` reads: a little-endian `u32` count, that many `u32` offsets,
//! then the type's name and each reference's field name, every one ending in
//! a zero byte.

use crate::{field_offsets, layout_of};
use klf_kir::{Type, TypeDeclaration, TypeKind};

/// The most references one map names; a larger payload is left untraced.
const MOST_EDGES: usize = 4096;

/// The trace map of a counted payload of type `ty`, when it holds a `ref`
/// in a struct field or a fixed array element. Enum payloads, `uniq` boxes
/// and `weak` references are not traced.
#[must_use]
pub fn trace_map(ty: &Type, declarations: &[TypeDeclaration]) -> Option<Vec<u8>> {
    let mut edges = Vec::new();
    if !collect(ty, 0, "value", declarations, &mut edges, 0) || edges.is_empty() {
        return None;
    }
    let count = u32::try_from(edges.len()).ok()?;
    let mut bytes = count.to_le_bytes().to_vec();
    for (offset, _) in &edges {
        bytes.extend(u32::try_from(*offset).ok()?.to_le_bytes());
    }
    bytes.extend(type_name(ty).bytes());
    bytes.push(0);
    for (_, field) in &edges {
        bytes.extend(field.bytes());
        bytes.push(0);
    }
    Some(bytes)
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Nominal(name, _) => name.as_str().to_owned(),
        other => other.to_string(),
    }
}

/// Adds the references of a `ty` at `offset`, named under `path`. Answers
/// false past the edge limit or the nesting limit.
fn collect(
    ty: &Type,
    offset: u64,
    path: &str,
    declarations: &[TypeDeclaration],
    edges: &mut Vec<(u64, String)>,
    depth: usize,
) -> bool {
    if depth > 32 || edges.len() > MOST_EDGES {
        return false;
    }
    match ty {
        Type::Reference(_) => edges.push((offset, path.to_owned())),
        Type::Nominal(name, _) => {
            let Some(declaration) = declarations.iter().find(|d| d.name == *name) else {
                return true;
            };
            if declaration.kind != TypeKind::Struct {
                return true;
            }
            let offsets = field_offsets(name.as_str(), declarations);
            for (field, at) in declaration.fields.iter().zip(offsets) {
                let inner = if depth == 0 {
                    field.name.as_str().to_owned()
                } else {
                    format!("{path}.{}", field.name)
                };
                if !collect(
                    &field.ty,
                    offset + at,
                    &inner,
                    declarations,
                    edges,
                    depth + 1,
                ) {
                    return false;
                }
            }
        }
        Type::Array(count, element) => {
            let layout = layout_of(element, declarations);
            let stride = layout.size.div_ceil(layout.align.max(1)) * layout.align.max(1);
            for index in 0..*count {
                let inner = format!("{path}[{index}]");
                let at = offset + index * stride;
                if !collect(element, at, &inner, declarations, edges, depth + 1) {
                    return false;
                }
            }
        }
        _ => {}
    }
    true
}
