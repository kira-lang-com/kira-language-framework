//! Memberwise `copy(dst: ptr, src: ptr)` and `destroy(obj: ptr)` functions
//! (section 11.2) for definition-built types with managed members.
//!
//! A struct is handled field by field. An enum dispatches on its tag and
//! handles the one payload of the live variant; `copy` then sets the
//! destination's tag.

use klf_core::{FileId, Name};
use klf_kir::{BlockTarget, Function, LoadMode, StoreMode, Type, TypeDeclaration};
use klf_services::FunctionBuilder;
use klf_source::Span;

/// How one member is copied and destroyed.
#[derive(Clone, Copy)]
pub(crate) enum Member {
    Trivial,
    /// `ref T`: copy retains, destroy releases.
    Counted,
    /// `uniq T`: no copy, destroy frees.
    Unique,
    /// A declared type with its own memberwise functions, by index.
    Nested(usize),
}

pub(crate) fn destroy_name(ty: &Name) -> Name {
    Name::new(format!("__klf_destroy_{ty}"))
}

pub(crate) fn copy_name(ty: &Name) -> Name {
    Name::new(format!("__klf_copy_{ty}"))
}

fn span() -> Span {
    Span::empty(FileId::from_u32(0), 0)
}

fn nominal_address(declaration: &TypeDeclaration) -> Type {
    Type::Address(Box::new(Type::Nominal(
        declaration.name.clone(),
        Vec::new(),
    )))
}

pub(crate) fn destroy_function(
    declaration: &TypeDeclaration,
    members: &[Vec<Member>],
    names: &[Name],
) -> Result<Function, String> {
    let object = Name::new("object");
    let mut fb = FunctionBuilder::new(
        destroy_name(&declaration.name),
        &[(object.clone(), Type::Pointer)],
        Type::Unit,
        span(),
    );
    fb.set_linkage(klf_kir::Linkage::Module);
    let typed = fb.convert(
        klf_kir::ConvertKind::Bitcast,
        object,
        nominal_address(declaration),
    );
    if declaration.variants.is_empty() {
        for (index, slot) in members.iter().enumerate() {
            let field = u32::try_from(index).map_err(|_| "too many fields".to_owned())?;
            if let Some(member) = slot.first().filter(|member| !is_trivial(**member)) {
                let address = fb.field_addr(typed.clone(), field);
                destroy_member(&mut fb, *member, address, names);
            }
        }
    } else {
        per_variant(&mut fb, &typed, members, |fb, variant, member| {
            let payload = fb.enum_payload_addr(typed.clone(), variant);
            destroy_member(fb, member, payload, names);
        })?;
    }
    fb.ret(None);
    Ok(fb.finish())
}

pub(crate) fn copy_function(
    declaration: &TypeDeclaration,
    members: &[Vec<Member>],
    names: &[Name],
    types: &[&TypeDeclaration],
) -> Result<Function, String> {
    let destination = Name::new("destination");
    let source = Name::new("source");
    let mut fb = FunctionBuilder::new(
        copy_name(&declaration.name),
        &[
            (destination.clone(), Type::Pointer),
            (source.clone(), Type::Pointer),
        ],
        Type::Unit,
        span(),
    );
    fb.set_linkage(klf_kir::Linkage::Module);
    let ty = nominal_address(declaration);
    let destination = fb.convert(klf_kir::ConvertKind::Bitcast, destination, ty.clone());
    let source = fb.convert(klf_kir::ConvertKind::Bitcast, source, ty);
    if declaration.variants.is_empty() {
        for (index, slot) in members.iter().enumerate() {
            let field = u32::try_from(index).map_err(|_| "too many fields".to_owned())?;
            let member = slot.first().copied().unwrap_or(Member::Trivial);
            let to = fb.field_addr(destination.clone(), field);
            let from = fb.field_addr(source.clone(), field);
            let ty = &declaration.fields[index].ty;
            copy_member(&mut fb, member, ty, to, from, names, types)?;
        }
    } else {
        let variants = members.len();
        let mut failure = None;
        per_variant_all(&mut fb, &source, variants, |fb, variant| {
            let index = variant as usize;
            if let (Some(member), Some(ty)) = (
                members[index].first().copied(),
                declaration.variants[index].payload.first(),
            ) {
                let to = fb.enum_payload_addr(destination.clone(), variant);
                let from = fb.enum_payload_addr(source.clone(), variant);
                if let Err(error) = copy_member(fb, member, ty, to, from, names, types) {
                    failure = Some(error);
                }
            }
            fb.enum_inject(destination.clone(), variant);
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
    }
    fb.ret(None);
    Ok(fb.finish())
}

fn is_trivial(member: Member) -> bool {
    matches!(member, Member::Trivial)
}

fn destroy_member(fb: &mut FunctionBuilder, member: Member, address: Name, names: &[Name]) {
    match member {
        Member::Trivial => {}
        Member::Counted | Member::Unique => {
            let value = fb.load_with(LoadMode::Take, address);
            fb.destroy(value);
        }
        Member::Nested(index) => {
            let raw = fb.convert(klf_kir::ConvertKind::Bitcast, address, Type::Pointer);
            fb.call_void(destroy_name(&names[index]), vec![raw]);
        }
    }
}

fn copy_member(
    fb: &mut FunctionBuilder,
    member: Member,
    ty: &Type,
    to: Name,
    from: Name,
    names: &[Name],
    types: &[&TypeDeclaration],
) -> Result<(), String> {
    match member {
        Member::Trivial => copy_trivial(fb, ty, to, from, types)?,
        Member::Counted | Member::Unique => {
            let value = fb.load_with(LoadMode::Copy, from);
            fb.store(StoreMode::Init, value, to);
        }
        Member::Nested(index) => {
            let to = fb.convert(klf_kir::ConvertKind::Bitcast, to, Type::Pointer);
            let from = fb.convert(klf_kir::ConvertKind::Bitcast, from, Type::Pointer);
            fb.call_void(copy_name(&names[index]), vec![to, from]);
        }
    }
    Ok(())
}

/// Copies a trivially copyable value leaf by leaf, since loads and stores
/// move scalars only.
fn copy_trivial(
    fb: &mut FunctionBuilder,
    ty: &Type,
    to: Name,
    from: Name,
    types: &[&TypeDeclaration],
) -> Result<(), String> {
    match ty {
        Type::Integer(_)
        | Type::Float(_)
        | Type::Pointer
        | Type::Address(_)
        | Type::Function(_) => {
            let value = fb.load(from);
            fb.store(StoreMode::Trivial, value, to);
        }
        Type::Unit | Type::Never => {}
        Type::Array(length, element) => {
            for index in 0..*length {
                let index = fb.const_int(Type::Integer(64), i128::from(index));
                let to = fb.elem_addr(to.clone(), index.clone());
                let from = fb.elem_addr(from.clone(), index);
                copy_trivial(fb, element, to, from, types)?;
            }
        }
        Type::Nominal(name, arguments) if arguments.is_empty() => {
            let declaration = types
                .iter()
                .find(|declaration| declaration.name == *name)
                .ok_or_else(|| format!("memberwise copy cannot find `%{name}`"))?;
            if declaration.variants.is_empty() {
                for (index, field) in declaration.fields.iter().enumerate() {
                    let index = u32::try_from(index).map_err(|_| "too many fields".to_owned())?;
                    let to = fb.field_addr(to.clone(), index);
                    let from = fb.field_addr(from.clone(), index);
                    copy_trivial(fb, &field.ty, to, from, types)?;
                }
            } else {
                let mut failure = None;
                per_variant_all(fb, &from, declaration.variants.len(), |fb, variant| {
                    if let Some(payload) = declaration.variants[variant as usize].payload.first() {
                        let to_payload = fb.enum_payload_addr(to.clone(), variant);
                        let from_payload = fb.enum_payload_addr(from.clone(), variant);
                        if let Err(error) =
                            copy_trivial(fb, payload, to_payload, from_payload, types)
                        {
                            failure = Some(error);
                        }
                    }
                    fb.enum_inject(to.clone(), variant);
                })?;
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }
        other => return Err(format!("memberwise copy does not cover `{other}` members")),
    }
    Ok(())
}

/// Runs `body` in a block for each variant whose payload is managed.
fn per_variant(
    fb: &mut FunctionBuilder,
    value: &Name,
    members: &[Vec<Member>],
    mut body: impl FnMut(&mut FunctionBuilder, u32, Member),
) -> Result<(), String> {
    let join = fb.new_block(&[]);
    let mut cases = Vec::new();
    let mut bodies = Vec::new();
    for (index, payload) in members.iter().enumerate() {
        let variant = u32::try_from(index).map_err(|_| "too many variants".to_owned())?;
        let block = fb.new_block(&[]);
        cases.push((
            variant,
            BlockTarget {
                block: block.clone(),
                arguments: Vec::new(),
            },
        ));
        bodies.push((block, variant, payload.first().copied()));
    }
    fb.switch_enum(value.clone(), cases, None);
    for (block, variant, member) in bodies {
        fb.switch_to(&block);
        if let Some(member) = member.filter(|member| !is_trivial(*member)) {
            body(fb, variant, member);
        }
        fb.branch(join.clone(), Vec::new());
    }
    fb.switch_to(&join);
    Ok(())
}

/// Runs `body` in a block for every variant.
fn per_variant_all(
    fb: &mut FunctionBuilder,
    value: &Name,
    variants: usize,
    mut body: impl FnMut(&mut FunctionBuilder, u32),
) -> Result<(), String> {
    let join = fb.new_block(&[]);
    let mut cases = Vec::new();
    let mut blocks = Vec::new();
    for index in 0..variants {
        let variant = u32::try_from(index).map_err(|_| "too many variants".to_owned())?;
        let block = fb.new_block(&[]);
        cases.push((
            variant,
            BlockTarget {
                block: block.clone(),
                arguments: Vec::new(),
            },
        ));
        blocks.push((block, variant));
    }
    fb.switch_enum(value.clone(), cases, None);
    for (block, variant) in blocks {
        fb.switch_to(&block);
        body(fb, variant);
        fb.branch(join.clone(), Vec::new());
    }
    fb.switch_to(&join);
    Ok(())
}
