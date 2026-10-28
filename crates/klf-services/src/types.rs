//! Session-owned type, layout and policy handles from section 8.5.

use crate::Session;
use klf_core::Name;
pub use klf_core::TypeId;
pub use klf_kir::{CopyPolicy, DestroyPolicy, LayoutRule, Pairing, Policy, RelocatePolicy};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionSignature {
    pub parameters: Vec<TypeId>,
    pub result: TypeId,
    pub failure: Option<TypeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeKind {
    Void,
    Integer { bits: u16, signed: bool },
    Float { bits: u16 },
    Pointer,
    Address(TypeId),
    Vector { element: TypeId, count: u32 },
    Tuple(Vec<TypeId>),
    Array { element: TypeId, count: u64 },
    Function(FunctionSignature),
    Nominal(Name),
    Generic(Name),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeLayout {
    Struct {
        fields: Vec<TypeId>,
        rule: LayoutRule,
    },
    Enum {
        variants: Vec<Vec<TypeId>>,
        rule: LayoutRule,
    },
    Explicit {
        size: u64,
        align: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeLayoutInfo {
    pub size: u64,
    pub align: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeInfo {
    pub kind: TypeKind,
    pub layout: Option<TypeLayout>,
    pub policy: Option<Policy>,
}

#[derive(Clone)]
pub(crate) struct TypeStore {
    types: Vec<TypeInfo>,
}

impl TypeStore {
    pub(crate) const fn new() -> Self {
        Self { types: Vec::new() }
    }

    fn alloc(&mut self, info: TypeInfo) -> TypeId {
        let id = TypeId::from_u32(u32::try_from(self.types.len()).expect("type count fits u32"));
        self.types.push(info);
        id
    }

    fn get(&self, id: TypeId) -> Option<&TypeInfo> {
        self.types.get(id.index())
    }

    fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeInfo> {
        self.types.get_mut(id.index())
    }
}

impl Session {
    #[must_use]
    pub fn void_type(&mut self) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Void,
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn int_type(&mut self, bits: u16, signed: bool) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Integer { bits, signed },
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn float_type(&mut self, bits: u16) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Float { bits },
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn pointer_type(&mut self) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Pointer,
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn address_type(&mut self, pointee: TypeId) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Address(pointee),
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn vector_type(&mut self, element: TypeId, count: u32) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Vector { element, count },
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn tuple_type(&mut self, elements: Vec<TypeId>) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Tuple(elements),
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn array_type(&mut self, element: TypeId, count: u64) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Array { element, count },
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn function_type(&mut self, signature: FunctionSignature) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Function(signature),
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn generic_type(&mut self, name: Name) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Generic(name),
            layout: None,
            policy: None,
        })
    }

    #[must_use]
    pub fn nominal_type(&mut self, name: Name, layout: TypeLayout, policy: Policy) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Nominal(name),
            layout: Some(layout),
            policy: Some(policy),
        })
    }

    #[must_use]
    pub fn opaque_nominal(&mut self, name: Name) -> TypeId {
        self.type_store_mut().alloc(TypeInfo {
            kind: TypeKind::Nominal(name),
            layout: None,
            policy: None,
        })
    }

    /// Completes an opaque nominal type in place.
    ///
    /// # Panics
    /// Panics when `ty` is not a nominal type in this session.
    pub fn complete_nominal(&mut self, ty: TypeId, layout: TypeLayout, policy: Policy) {
        let info = self
            .type_store_mut()
            .get_mut(ty)
            .unwrap_or_else(|| panic!("invalid type {}", ty.as_u32()));
        assert!(
            matches!(info.kind, TypeKind::Nominal(_)),
            "type is not nominal"
        );
        info.layout = Some(layout);
        info.policy = Some(policy);
    }

    #[must_use]
    pub fn type_info(&self, ty: TypeId) -> Option<&TypeInfo> {
        self.type_store().get(ty)
    }

    #[must_use]
    pub fn type_layout(&self, ty: TypeId) -> Option<&TypeLayout> {
        self.type_info(ty)?.layout.as_ref()
    }

    #[must_use]
    pub fn type_policy(&self, ty: TypeId) -> Option<&Policy> {
        self.type_info(ty)?.policy.as_ref()
    }

    #[must_use]
    pub fn layout_of(&self, ty: TypeId) -> Option<TypeLayoutInfo> {
        self.layout_of_inner(ty, &mut HashSet::new())
    }

    #[must_use]
    pub fn size_of(&self, ty: TypeId) -> Option<u64> {
        self.layout_of(ty).map(|layout| layout.size)
    }

    #[must_use]
    pub fn align_of(&self, ty: TypeId) -> Option<u64> {
        self.layout_of(ty).map(|layout| layout.align)
    }

    fn layout_of_inner(
        &self,
        ty: TypeId,
        visiting: &mut HashSet<TypeId>,
    ) -> Option<TypeLayoutInfo> {
        if !visiting.insert(ty) {
            return None;
        }
        let result = match &self.type_info(ty)?.kind {
            TypeKind::Void => Some(TypeLayoutInfo { size: 0, align: 1 }),
            TypeKind::Integer { bits, .. } | TypeKind::Float { bits } => {
                let bytes = u64::from(bits.div_ceil(8)).max(1);
                Some(TypeLayoutInfo {
                    size: bytes,
                    align: bytes,
                })
            }
            TypeKind::Pointer | TypeKind::Address(_) | TypeKind::Function(_) => {
                let bytes = u64::from(self.target().pointer_width() / 8);
                Some(TypeLayoutInfo {
                    size: bytes,
                    align: bytes,
                })
            }
            TypeKind::Vector { element, count } => {
                let element = self.layout_of_inner(*element, visiting)?;
                Some(TypeLayoutInfo {
                    size: element.size * u64::from(*count),
                    align: element.align.max(1),
                })
            }
            TypeKind::Tuple(elements) => self.aggregate_layout(elements, visiting),
            TypeKind::Array { element, count } => {
                let element = self.layout_of_inner(*element, visiting)?;
                let stride = round_up(element.size, element.align);
                Some(TypeLayoutInfo {
                    size: stride * *count,
                    align: element.align.max(1),
                })
            }
            TypeKind::Nominal(_) => {
                let layout = self.type_layout(ty)?.clone();
                self.explicit_layout(&layout, visiting)
            }
            TypeKind::Generic(_) => None,
        };
        visiting.remove(&ty);
        result
    }

    fn explicit_layout(
        &self,
        layout: &TypeLayout,
        visiting: &mut HashSet<TypeId>,
    ) -> Option<TypeLayoutInfo> {
        match layout {
            TypeLayout::Struct { fields, .. } => self.aggregate_layout(fields, visiting),
            TypeLayout::Enum { variants, .. } => {
                let tag_size = enum_tag_bytes(variants.len());
                // As large as the largest variant and aligned as the most
                // aligned, which need not be the same variant.
                let payload = variants
                    .iter()
                    .map(|variant| self.aggregate_layout(variant, visiting))
                    .collect::<Option<Vec<_>>>()?
                    .into_iter()
                    .fold(TypeLayoutInfo { size: 0, align: 1 }, |area, layout| {
                        TypeLayoutInfo {
                            size: area.size.max(layout.size),
                            align: area.align.max(layout.align),
                        }
                    });
                let payload_offset = round_up(tag_size, payload.align);
                let align = tag_size.max(payload.align).max(1);
                Some(TypeLayoutInfo {
                    size: round_up(payload_offset + payload.size, align),
                    align,
                })
            }
            TypeLayout::Explicit { size, align } => Some(TypeLayoutInfo {
                size: *size,
                align: (*align).max(1),
            }),
        }
    }

    fn aggregate_layout(
        &self,
        fields: &[TypeId],
        visiting: &mut HashSet<TypeId>,
    ) -> Option<TypeLayoutInfo> {
        let mut offset = 0;
        let mut align = 1;
        for field in fields {
            let layout = self.layout_of_inner(*field, visiting)?;
            offset = round_up(offset, layout.align);
            offset += layout.size;
            align = align.max(layout.align.max(1));
        }
        Some(TypeLayoutInfo {
            size: round_up(offset, align),
            align,
        })
    }
}

const fn round_up(value: u64, align: u64) -> u64 {
    if align == 0 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

const fn enum_tag_bytes(case_count: usize) -> u64 {
    if case_count <= 256 {
        1
    } else if case_count <= 65_536 {
        2
    } else {
        4
    }
}
