//! Types (section 8): interned from their KSIR spelling, arrays strided by a
//! space's layout rule, and structs placed by the rule or by the definition.

use super::{Args, PendingMember, ShaderState};
use klf_shader::layout::MemberSpec;
use klf_shader::parse::fragment;
use klf_shader::{ArraySize, LayoutRule, StructMember};

fn word(value: impl TryInto<u64>) -> Result<u64, String> {
    value
        .try_into()
        .map_err(|_| "a handle out of range".to_owned())
}

pub(super) fn rule(text: &str) -> Result<LayoutRule, String> {
    Ok(match text {
        "std140" => LayoutRule::Std140,
        "std430" => LayoutRule::Std430,
        "scalar" => LayoutRule::Scalar,
        "msl" => LayoutRule::Msl,
        "explicit" => LayoutRule::Explicit,
        "none" => LayoutRule::None,
        other => return Err(format!("unknown layout rule `{other}`")),
    })
}

impl ShaderState {
    /// `klf.shader.type(module, text)`: any type but a struct, or `%Name`.
    pub(super) fn type_text(&mut self, args: &Args) -> Result<u64, String> {
        let build = self.build(args.word(0)?)?;
        let handle = fragment::ty(&mut build.builder.module, args.text(0)?)?;
        word(handle.raw())
    }

    /// `klf.shader.type.array(module, base, count, space)`: `count` `-1`
    /// is runtime-sized; the stride is the one `space`'s rule gives.
    pub(super) fn type_array(&mut self, args: &Args) -> Result<u64, String> {
        let base = args.handle(1)?;
        let size = match args.int(2)? {
            -1 => ArraySize::Runtime,
            count => ArraySize::Constant(
                u32::try_from(count).map_err(|_| format!("an array of {count} elements"))?,
            ),
        };
        let space = fragment::space(args.text(0)?)?;
        let build = self.build(args.word(0)?)?;
        let handle = build
            .builder
            .array_type(base, size, space)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.type.member(module, name, type, binding, offset, size,
    /// align)`: one member of the next struct. Offset `-1` leaves placement
    /// to the layout rule.
    pub(super) fn type_member(&mut self, args: &Args) -> Result<u64, String> {
        let span = self.build(args.word(0)?)?.span;
        let member = PendingMember {
            name: args.text(0)?.to_owned(),
            ty: args.handle(1)?,
            binding: fragment::binding(args.text(1)?)?,
            offset: args.int(2)?,
            size: args.small(3)?,
            align: args.small(4)?,
            span,
        };
        self.build(args.word(0)?)?.members.push(member);
        Ok(0)
    }

    /// `klf.shader.type.struct(module, name, space)`: the pending members
    /// placed by the rule `space` takes (section 6.5).
    pub(super) fn type_struct(&mut self, args: &Args) -> Result<u64, String> {
        let name = args.text(0)?;
        let space = fragment::space(args.text(1)?)?;
        let span = self.build(args.word(0)?)?.span;
        let build = self.build(args.word(0)?)?;
        let members = std::mem::take(&mut build.members)
            .into_iter()
            .map(|member| MemberSpec {
                name: member.name,
                ty: member.ty,
                binding: member.binding,
                span: member.span,
            })
            .collect();
        let handle = build
            .builder
            .struct_type(name, members, space, span)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.type.struct_explicit(module, name, size, align)`: the
    /// pending members at the offsets the definition computed.
    pub(super) fn type_struct_explicit(&mut self, args: &Args) -> Result<u64, String> {
        let name = args.text(0)?;
        let (size, align) = (args.small(1)?, args.small(2)?);
        let build = self.build(args.word(0)?)?;
        let span = build.span;
        let mut members = Vec::new();
        for member in std::mem::take(&mut build.members) {
            let offset = u32::try_from(member.offset)
                .map_err(|_| format!("member `{}` of `{name}` has no offset", member.name))?;
            members.push(StructMember {
                name: member.name,
                ty: member.ty,
                binding: member.binding,
                offset,
                size: member.size,
                align: member.align,
                span: member.span,
            });
        }
        let handle = build
            .builder
            .struct_type_explicit(name, members, size, align, span)
            .map_err(|error| error.0)?;
        word(handle.raw())
    }

    /// `klf.shader.type.size(module, type, rule)`.
    pub(super) fn type_size(&mut self, args: &Args) -> Result<u64, String> {
        let rule = rule(args.text(0)?)?;
        let ty = args.handle(1)?;
        let build = self.build(args.word(0)?)?;
        if build.builder.module.types.get(ty).is_none() {
            return Err("an unknown type".to_owned());
        }
        Ok(u64::from(build.builder.extent(ty, rule).0))
    }

    /// `klf.shader.type.align(module, type, rule)`.
    pub(super) fn type_align(&mut self, args: &Args) -> Result<u64, String> {
        let rule = rule(args.text(0)?)?;
        let ty = args.handle(1)?;
        let build = self.build(args.word(0)?)?;
        if build.builder.module.types.get(ty).is_none() {
            return Err("an unknown type".to_owned());
        }
        Ok(u64::from(build.builder.extent(ty, rule).1))
    }
}
