use super::{LowerError, Lowerer, err};
use crate::bytecode::{Instr, Opcode, WidthCode, arith_flags};
use crate::program::BranchCondition;
use klf_core::Name;
use klf_kir::{BlockTarget, Type};

impl Lowerer<'_> {
    pub(super) fn lower_enum_tag(
        &mut self,
        results: &[Name],
        value: &Name,
    ) -> Result<(), LowerError> {
        let (_, bits) = self.enum_info(value)?;
        let dest = self.result(results)?;
        let source = self.register(value)?;
        let width = WidthCode::from_bits(bits).ok_or_else(|| err("invalid enum tag width"))?;
        self.emit(Instr::abc(Opcode::Load, dest, source, 0).with_flags(width.code()));
        Ok(())
    }

    pub(super) fn lower_enum_payload_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        variant: u32,
    ) -> Result<(), LowerError> {
        let (name, _) = self.enum_info(base)?;
        let offset = klf_layout::enum_payload_offset(name, variant, self.decls)
            .ok_or_else(|| err("enum variant has no payload area"))?;
        let dest = self.result(results)?;
        let base_reg = self.register(base)?;
        let offset_reg = self.load_u64_const(offset)?;
        self.emit(
            Instr::abc(Opcode::Add, dest, base_reg, offset_reg).with_flags(arith_flags(
                WidthCode::W64,
                false,
                crate::bytecode::OverflowMode::Wrap,
            )),
        );
        Ok(())
    }

    pub(super) fn lower_enum_inject(
        &mut self,
        address: &Name,
        variant: u32,
    ) -> Result<(), LowerError> {
        let (_, bits) = self.enum_info(address)?;
        let address_reg = self.register(address)?;
        let tag_reg = self.load_u64_const(u64::from(variant))?;
        let width = WidthCode::from_bits(bits).ok_or_else(|| err("invalid enum tag width"))?;
        self.emit(Instr::abc(Opcode::Store, address_reg, tag_reg, 0).with_flags(width.code()));
        Ok(())
    }

    pub(super) fn lower_enum_switch(
        &mut self,
        value: &Name,
        cases: &[(u32, BlockTarget)],
        default: Option<&BlockTarget>,
    ) -> Result<(), LowerError> {
        let (_, bits) = self.enum_info(value)?;
        let selector = self.fresh()?;
        let source = self.register(value)?;
        let width = WidthCode::from_bits(bits).ok_or_else(|| err("invalid enum tag width"))?;
        self.emit(Instr::abc(Opcode::Load, selector, source, 0).with_flags(width.code()));
        for (variant, target) in cases {
            let key = self.load_u64_const(u64::from(*variant))?;
            let comparison = self.fresh()?;
            self.emit(Instr::abc(Opcode::Icmp, comparison, selector, key).with_flags(width.code()));
            let site =
                self.add_branch_site(Some(BranchCondition::Register(comparison)), target, None)?;
            self.emit(Instr::from_bx(Opcode::BrIf, 0, site));
        }
        if let Some(default) = default {
            let site = self.add_branch_site(None, default, None)?;
            self.emit(Instr::from_bx(Opcode::Br, 0, site));
        } else {
            self.emit(Instr::abc(Opcode::Unreach, 0, 0, 0));
        }
        Ok(())
    }

    fn enum_info(&self, value: &Name) -> Result<(&str, u16), LowerError> {
        let ty = self
            .types
            .get(value)
            .ok_or_else(|| err(format!("cannot type `%{value}`")))?;
        let (Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner)) = ty else {
            return Err(err("enum operation requires an enum address"));
        };
        let Type::Nominal(name, _) = inner.as_ref() else {
            return Err(err("enum operation requires a nominal enum"));
        };
        let declaration = self
            .decls
            .iter()
            .find(|declaration| declaration.name == *name)
            .filter(|declaration| !declaration.variants.is_empty())
            .ok_or_else(|| err("unknown enum declaration"))?;
        Ok((
            declaration.name.as_str(),
            klf_layout::enum_declaration_tag_bits(declaration),
        ))
    }
}
