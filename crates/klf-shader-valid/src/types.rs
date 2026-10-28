//! Type and global rules: SV2, SV3, SV4, SV14, SV15, SV16, the
//! non-resource half of SV13, and the type half of SV24.

use crate::ValidationError;
use klf_shader::layout::{MemberSpec, array_stride, extent, host_shareable, place_members};
use klf_shader::{
    AddressSpace, ArraySize, Capability, Handle, ImageClass, ImageDim, LayoutRule, Module,
    ScalarKind, StorageFormat, Type, TypeInner,
};

fn scalar_capability(scalar: klf_shader::Scalar) -> Option<Capability> {
    match (scalar.kind, scalar.width) {
        (ScalarKind::Float, 2) => Some(Capability::Float16),
        (ScalarKind::Float, 8) => Some(Capability::Float64),
        (ScalarKind::Sint | ScalarKind::Uint, 8) => Some(Capability::Int64),
        _ => None,
    }
}

fn scalar_width_valid(scalar: klf_shader::Scalar) -> bool {
    match scalar.kind {
        ScalarKind::Bool => scalar.width == 1,
        ScalarKind::Sint | ScalarKind::Uint => matches!(scalar.width, 4 | 8),
        ScalarKind::Float => matches!(scalar.width, 2 | 4 | 8),
    }
}

/// The capabilities a type needs (SV24).
pub(crate) fn type_capabilities(inner: &TypeInner) -> Vec<Capability> {
    let mut needed = Vec::new();
    if let Some(scalar) = inner.scalar() {
        needed.extend(scalar_capability(scalar));
    }
    match inner {
        TypeInner::Atomic(scalar) if scalar.width == 8 => needed.push(Capability::Int64Atomics),
        TypeInner::Image {
            dim,
            arrayed,
            class,
        } => {
            if *dim == ImageDim::Cube && *arrayed {
                needed.push(Capability::CubeArray);
            }
            match class {
                ImageClass::Sampled {
                    multisampled: true, ..
                }
                | ImageClass::Depth { multisampled: true } => {
                    needed.push(Capability::Multisampled);
                }
                ImageClass::Storage { access, .. } if access.load && access.store => {
                    needed.push(Capability::StorageImageReadWrite);
                }
                _ => {}
            }
            if let ImageClass::Storage { access, .. } = class
                && access.atomic
            {
                needed.push(Capability::ImageAtomics);
            }
        }
        _ => {}
    }
    needed
}

fn check_shape(module: &Module, handle: Handle<Type>, problems: &mut Vec<ValidationError>) {
    let types = &module.types;
    let span = types.span(handle);
    let mut fail = |message: String| problems.push(ValidationError::new("SV2", message, span));
    let earlier = |inner: Handle<Type>| inner.index() < handle.index();
    match &types[handle].inner {
        TypeInner::Scalar(scalar) if !scalar_width_valid(*scalar) => {
            fail(format!("scalar `{}` has an invalid width", scalar.text()));
        }
        TypeInner::Vector { size, scalar } => {
            if !(2..=4).contains(size) || !scalar_width_valid(*scalar) {
                fail(format!("vector of {size} `{}` is malformed", scalar.text()));
            }
        }
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => {
            if !(2..=4).contains(columns)
                || !(2..=4).contains(rows)
                || scalar.kind != ScalarKind::Float
            {
                fail(format!(
                    "matrix {columns}x{rows} of `{}` is malformed",
                    scalar.text()
                ));
            }
        }
        TypeInner::Atomic(scalar) => {
            if !matches!(scalar.kind, ScalarKind::Sint | ScalarKind::Uint)
                || !matches!(scalar.width, 4 | 8)
            {
                fail(format!("atomic of `{}`", scalar.text()));
            }
        }
        TypeInner::Pointer { base, .. } if !earlier(*base) => {
            fail("a pointer names a later type".to_owned());
        }
        TypeInner::Array { base, size, stride } => {
            if !earlier(*base) {
                fail("an array names a later type".to_owned());
                return;
            }
            if *size == ArraySize::Constant(0) {
                fail("an array of size zero".to_owned());
            }
            if let ArraySize::Override(item) = size
                && module.overrides.get(*item).is_none()
            {
                fail("an array sized by an unknown override".to_owned());
            }
            let (element, align) = extent(types, *base, LayoutRule::Scalar);
            if *stride < element || *stride % align.max(1) != 0 {
                fail(format!(
                    "array stride {stride} is below the element size {element} or not a multiple of {align}"
                ));
            }
            if let TypeInner::Array {
                size: ArraySize::Runtime,
                ..
            } = types[*base].inner
            {
                fail("an array of runtime-sized arrays".to_owned());
            }
        }
        TypeInner::Image {
            dim,
            arrayed,
            class: ImageClass::Storage { format, access },
        } => {
            if *dim == ImageDim::D3 && *arrayed {
                fail("a 3d image cannot be arrayed".to_owned());
            }
            if !(access.load || access.store || access.atomic) {
                fail("a storage image with no access".to_owned());
            }
            if access.atomic && !matches!(format, StorageFormat::R32Uint | StorageFormat::R32Sint) {
                fail("atomic access needs format r32uint or r32sint".to_owned());
            }
        }
        TypeInner::Image {
            dim: ImageDim::D3,
            arrayed: true,
            ..
        } => fail("a 3d image cannot be arrayed".to_owned()),
        _ => {}
    }
}

fn check_struct(module: &Module, handle: Handle<Type>, problems: &mut Vec<ValidationError>) {
    let types = &module.types;
    let span = types.span(handle);
    let ty = &types[handle];
    let TypeInner::Struct {
        rule,
        members,
        size,
        align,
    } = &ty.inner
    else {
        return;
    };
    let name = ty.name.as_deref().unwrap_or("struct");
    let mut fail = |rule: &'static str, message: String| {
        problems.push(ValidationError::new(rule, message, span));
    };
    let mut end = 0;
    let mut widest = 1;
    for (index, member) in members.iter().enumerate() {
        if member.ty.index() >= handle.index() {
            fail(
                "SV2",
                format!("member `{}` of `{name}` names a later type", member.name),
            );
            return;
        }
        if let TypeInner::Array {
            size: ArraySize::Runtime,
            ..
        } = types[member.ty].inner
            && index + 1 != members.len()
        {
            fail(
                "SV3",
                format!(
                    "runtime-sized member `{}` of `{name}` is not last",
                    member.name
                ),
            );
        }
        if member.align == 0 || member.offset % member.align != 0 {
            fail(
                "SV3",
                format!(
                    "member `{}` of `{name}` is not aligned to {}",
                    member.name, member.align
                ),
            );
        }
        if member.offset < end {
            fail(
                "SV3",
                format!(
                    "member `{}` of `{name}` overlaps the previous one",
                    member.name
                ),
            );
        }
        if *rule != LayoutRule::None {
            let (natural, natural_align) = extent(types, member.ty, LayoutRule::Scalar);
            let runtime = matches!(
                types[member.ty].inner,
                TypeInner::Array {
                    size: ArraySize::Runtime,
                    ..
                }
            );
            if (member.size < natural && !runtime) || member.align % natural_align.max(1) != 0 {
                fail(
                    "SV3",
                    format!(
                        "member `{}` of `{name}` records a smaller extent than its type",
                        member.name
                    ),
                );
            }
        }
        end = member.offset + member.size;
        widest = widest.max(member.align);
    }
    if *align == 0 || *align % 4.min(widest) != 0 || *size < end || *size % *align != 0 {
        fail(
            "SV3",
            format!(
                "`{name}` has size {size} and alignment {align}, which do not cover its members"
            ),
        );
    }
    if matches!(rule, LayoutRule::Explicit | LayoutRule::None) {
        return;
    }
    let specs = members
        .iter()
        .map(|member| MemberSpec {
            name: member.name.clone(),
            ty: member.ty,
            binding: member.binding,
            span: member.span,
        })
        .collect();
    let (placed, expected_size, expected_align) = place_members(types, specs, *rule);
    let matches = placed.iter().zip(members).all(|(placed, member)| {
        placed.offset == member.offset && placed.size == member.size && placed.align == member.align
    });
    if !matches || expected_size != *size || expected_align != *align {
        fail(
            "SV4",
            format!("`{name}` does not match rule `{}`", rule.name()),
        );
    }
    for member in members {
        if let TypeInner::Array { base, stride, .. } = types[member.ty].inner
            && stride != array_stride(types, base, *rule)
        {
            fail(
                "SV4",
                format!(
                    "array member `{}` of `{name}` has a stride rule `{}` does not give",
                    member.name,
                    rule.name()
                ),
            );
        }
    }
}

/// SV2, SV3, SV4 and the type half of SV24.
pub(crate) fn check_types(module: &Module, problems: &mut Vec<ValidationError>) {
    for (handle, ty) in module.types.iter() {
        check_shape(module, handle, problems);
        check_struct(module, handle, problems);
        for capability in type_capabilities(&ty.inner) {
            if !module.capabilities.contains(&capability) {
                problems.push(ValidationError::new(
                    "SV24",
                    format!("type needs capability `{}`", capability.name()),
                    module.types.span(handle),
                ));
            }
        }
    }
}

fn contains_bool(module: &Module, ty: Handle<Type>) -> bool {
    match &module.types[ty].inner {
        TypeInner::Scalar(scalar) | TypeInner::Vector { scalar, .. } => {
            scalar.kind == ScalarKind::Bool
        }
        TypeInner::Array { base, .. } => contains_bool(module, *base),
        TypeInner::Struct { members, .. } => members
            .iter()
            .any(|member| contains_bool(module, member.ty)),
        _ => false,
    }
}

fn contains_atomic(module: &Module, ty: Handle<Type>) -> bool {
    match &module.types[ty].inner {
        TypeInner::Atomic(_) => true,
        TypeInner::Array { base, .. } => contains_atomic(module, *base),
        TypeInner::Struct { members, .. } => members
            .iter()
            .any(|member| contains_atomic(module, member.ty)),
        _ => false,
    }
}

/// Whether `ty` is or ends in a runtime-sized array.
pub(crate) fn runtime_sized(module: &Module, ty: Handle<Type>) -> bool {
    match &module.types[ty].inner {
        TypeInner::Array {
            size: ArraySize::Runtime,
            ..
        } => true,
        TypeInner::Struct { members, .. } => members
            .last()
            .is_some_and(|member| runtime_sized(module, member.ty)),
        _ => false,
    }
}

/// SV13 for non-resources, SV14, SV15 and the memory half of SV16.
pub(crate) fn check_globals(module: &Module, problems: &mut Vec<ValidationError>) {
    for (handle, global) in module.globals.iter() {
        let span = module.globals.span(handle);
        let mut fail = |rule: &'static str, message: String| {
            problems.push(ValidationError::new(rule, message, span));
        };
        let name = &global.name;
        let inner = &module.types[global.ty].inner;
        if !global.space.is_resource() && global.binding.is_some() {
            fail(
                "SV13",
                format!("`{name}` is not a resource but has a binding"),
            );
        }
        if global.space.is_host_shared() {
            if !host_shareable(&module.types, global.ty) {
                fail(
                    "SV15",
                    format!("`{name}` has a type that is not host-shareable"),
                );
            }
            if contains_bool(module, global.ty) {
                fail(
                    "SV16",
                    format!("`{name}` holds a bool in host-shareable memory"),
                );
            }
        }
        if contains_atomic(module, global.ty)
            && !matches!(
                global.space,
                AddressSpace::Storage { .. } | AddressSpace::Workgroup
            )
        {
            fail(
                "SV14",
                format!("`{name}` holds an atomic outside storage and workgroup memory"),
            );
        }
        if contains_atomic(module, global.ty)
            && let AddressSpace::Storage { access } = global.space
            && !access.atomic
            && access.store
        {
            fail(
                "SV14",
                format!("`{name}` holds atomics but its access lacks `atomic`"),
            );
        }
        if inner.is_handle() != (global.space == AddressSpace::Handle) {
            fail(
                "SV14",
                format!("`{name}`: images and samplers live exactly in the handle space"),
            );
        }
        if runtime_sized(module, global.ty) && !matches!(global.space, AddressSpace::Storage { .. })
        {
            fail(
                "SV14",
                format!("`{name}` is runtime-sized outside storage memory"),
            );
        }
        if matches!(inner, TypeInner::Pointer { .. }) {
            fail("SV14", format!("`{name}` is a pointer"));
        }
        if let AddressSpace::Storage { access } = global.space
            && !access.load
        {
            fail("SV14", format!("storage `{name}` lacks `load` access"));
        }
        match (global.space, global.init) {
            (AddressSpace::Private, _) | (_, None) => {}
            (_, Some(_)) => fail(
                "SV14",
                format!("`{name}` has an initializer outside the private space"),
            ),
        }
    }
}
