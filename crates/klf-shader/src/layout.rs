//! Sizes, alignments, strides and struct offsets under each layout rule
//! (section 8.4).

use crate::Span;
use crate::arena::{Handle, UniqueArena};
use crate::types::{ArraySize, Binding, LayoutRule, StructMember, Type, TypeInner};

#[must_use]
pub const fn round_up(value: u32, align: u32) -> u32 {
    if align <= 1 {
        return value;
    }
    value.div_ceil(align) * align
}

/// The size and alignment of a vector of `size` components of `width`-byte
/// scalars under `rule`.
#[must_use]
pub const fn vector_extent(size: u8, width: u8, rule: LayoutRule) -> (u32, u32) {
    let n = width as u32;
    let size = size as u32;
    match rule {
        LayoutRule::Scalar => (size * n, n),
        LayoutRule::Msl => match size {
            2 => (2 * n, 2 * n),
            _ => (4 * n, 4 * n),
        },
        LayoutRule::Std140 | LayoutRule::Std430 | LayoutRule::Explicit | LayoutRule::None => {
            match size {
                2 => (2 * n, 2 * n),
                3 => (3 * n, 4 * n),
                _ => (4 * n, 4 * n),
            }
        }
    }
}

/// The size and alignment of `ty` under `rule`. Structs and arrays carry
/// their own extents; handle types have none.
#[must_use]
pub fn extent(types: &UniqueArena<Type>, ty: Handle<Type>, rule: LayoutRule) -> (u32, u32) {
    match &types[ty].inner {
        TypeInner::Scalar(scalar) | TypeInner::Atomic(scalar) => {
            (u32::from(scalar.width), u32::from(scalar.width))
        }
        TypeInner::Vector { size, scalar } => vector_extent(*size, scalar.width, rule),
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => matrix_extent(*columns, *rows, scalar.width, rule),
        TypeInner::Array { base, size, stride } => {
            let (_, align) = element_extent(types, *base, rule);
            let count = match size {
                ArraySize::Constant(count) => *count,
                ArraySize::Override(_) | ArraySize::Runtime => 1,
            };
            (count * stride, align)
        }
        TypeInner::Struct { size, align, .. } => (*size, *align),
        TypeInner::Pointer { .. } | TypeInner::Image { .. } | TypeInner::Sampler { .. } => (0, 1),
    }
}

/// A matrix's extent: its columns are arrays of `vecR` under the rule.
#[must_use]
pub const fn matrix_extent(columns: u8, rows: u8, width: u8, rule: LayoutRule) -> (u32, u32) {
    let (column_size, column_align) = vector_extent(rows, width, rule);
    let columns = columns as u32;
    match rule {
        LayoutRule::Std140 => {
            let align = if column_align > 16 { column_align } else { 16 };
            (columns * round_up(column_size, align), align)
        }
        LayoutRule::Scalar => (columns * rows as u32 * width as u32, width as u32),
        LayoutRule::Msl => (columns * column_size, column_align),
        LayoutRule::Std430 | LayoutRule::Explicit | LayoutRule::None => {
            (columns * round_up(column_size, column_align), column_align)
        }
    }
}

fn element_extent(types: &UniqueArena<Type>, base: Handle<Type>, rule: LayoutRule) -> (u32, u32) {
    let (size, align) = extent(types, base, rule);
    match rule {
        LayoutRule::Std140 => (size, align.max(16)),
        _ => (size, align),
    }
}

/// The stride of an array of `base` under `rule`.
#[must_use]
pub fn array_stride(types: &UniqueArena<Type>, base: Handle<Type>, rule: LayoutRule) -> u32 {
    let (size, align) = element_extent(types, base, rule);
    round_up(size, align)
}

/// A member before placement.
#[derive(Clone, Debug)]
pub struct MemberSpec {
    pub name: String,
    pub ty: Handle<Type>,
    pub binding: Option<Binding>,
    pub span: Span,
}

/// Places `members` in declaration order under `rule` and returns them with
/// the struct's size and alignment.
#[must_use]
pub fn place_members(
    types: &UniqueArena<Type>,
    members: Vec<MemberSpec>,
    rule: LayoutRule,
) -> (Vec<StructMember>, u32, u32) {
    let mut end = 0;
    let mut max_align = 1;
    let mut placed = Vec::with_capacity(members.len());
    for member in members {
        let (size, align) = extent(types, member.ty, rule);
        let align = align.max(1);
        let offset = round_up(end, align);
        end = offset + size;
        max_align = max_align.max(align);
        placed.push(StructMember {
            name: member.name,
            ty: member.ty,
            binding: member.binding,
            offset,
            size,
            align,
            span: member.span,
        });
    }
    let align = match rule {
        LayoutRule::Std140 => round_up(max_align, 16),
        _ => max_align,
    };
    (placed, round_up(end, align), align)
}

/// Whether `ty` is host-shareable (section 8.5).
#[must_use]
pub fn host_shareable(types: &UniqueArena<Type>, ty: Handle<Type>) -> bool {
    match &types[ty].inner {
        TypeInner::Scalar(scalar) | TypeInner::Vector { scalar, .. } => {
            scalar.kind != crate::types::ScalarKind::Bool
        }
        TypeInner::Matrix { .. } | TypeInner::Atomic(_) => true,
        TypeInner::Array { base, .. } => host_shareable(types, *base),
        TypeInner::Struct { rule, members, .. } => {
            *rule != LayoutRule::None
                && members
                    .iter()
                    .all(|member| host_shareable(types, member.ty))
        }
        TypeInner::Pointer { .. } | TypeInner::Image { .. } | TypeInner::Sampler { .. } => false,
    }
}

/// Whether a value of `ty` can be built and copied (section 8.2).
#[must_use]
pub fn constructible(types: &UniqueArena<Type>, ty: Handle<Type>) -> bool {
    match &types[ty].inner {
        TypeInner::Scalar(_) | TypeInner::Vector { .. } | TypeInner::Matrix { .. } => true,
        TypeInner::Array { base, size, .. } => {
            matches!(size, ArraySize::Constant(_)) && constructible(types, *base)
        }
        TypeInner::Struct { members, .. } => {
            members.iter().all(|member| constructible(types, member.ty))
        }
        TypeInner::Atomic(_)
        | TypeInner::Pointer { .. }
        | TypeInner::Image { .. }
        | TypeInner::Sampler { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;
    use crate::types::Scalar;

    fn ty(types: &mut UniqueArena<Type>, inner: TypeInner) -> Handle<Type> {
        types.insert(Type { name: None, inner }, Span::default())
    }

    #[test]
    fn rules_place_a_scalar_after_a_three_wide_vector() {
        let mut types = UniqueArena::new();
        let vec3 = ty(
            &mut types,
            TypeInner::Vector {
                size: 3,
                scalar: Scalar::F32,
            },
        );
        let float = ty(&mut types, TypeInner::Scalar(Scalar::F32));
        let mat3 = ty(
            &mut types,
            TypeInner::Matrix {
                columns: 3,
                rows: 3,
                scalar: Scalar::F32,
            },
        );
        let members = || {
            vec![
                MemberSpec {
                    name: "a".into(),
                    ty: vec3,
                    binding: None,
                    span: Span::default(),
                },
                MemberSpec {
                    name: "b".into(),
                    ty: float,
                    binding: None,
                    span: Span::default(),
                },
            ]
        };
        let expected = [
            (LayoutRule::Std140, 12, 16, 16),
            (LayoutRule::Std430, 12, 16, 16),
            (LayoutRule::Scalar, 12, 16, 4),
            (LayoutRule::Msl, 16, 32, 16),
        ];
        for (rule, offset, size, align) in expected {
            let (placed, total, struct_align) = place_members(&types, members(), rule);
            assert_eq!(
                (placed[1].offset, total, struct_align),
                (offset, size, align),
                "{rule:?}"
            );
        }
        assert_eq!(extent(&types, mat3, LayoutRule::Std140), (48, 16));
        assert_eq!(extent(&types, mat3, LayoutRule::Scalar), (36, 4));
        assert_eq!(array_stride(&types, float, LayoutRule::Std140), 16);
        assert_eq!(array_stride(&types, float, LayoutRule::Std430), 4);
    }
}
