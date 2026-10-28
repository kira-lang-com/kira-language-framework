use super::{FnLowerer, cl_type};
use crate::{EmitError, err};
use cranelift::codegen::ir::MemFlagsData;
use cranelift::prelude::{InstBuilder, IntCC, Value, types};
use klf_core::Name;
use klf_kir::{BlockTarget, Type};

impl FnLowerer<'_> {
    pub(super) fn enum_tag(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(value)?;
        let tag = self.enum_tag_value(value, bits)?;
        self.define(results, tag)
    }

    pub(super) fn enum_payload_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        variant: u32,
    ) -> Result<(), EmitError> {
        let (name, _) = self.enum_info(base)?;
        let offset = self
            .layouts
            .enum_payload_offset(name, variant)
            .ok_or_else(|| err("enum variant has no payload area"))?;
        let base_value = self.value(base)?;
        #[allow(clippy::cast_possible_wrap)]
        let offset_value = self.builder.ins().iconst(types::I64, offset as i64);
        let address = self.builder.ins().iadd(base_value, offset_value);
        self.define(results, address)
    }

    pub(super) fn enum_inject(&mut self, address: &Name, variant: u32) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(address)?;
        let pointer = self.value(address)?;
        let tag = self
            .builder
            .ins()
            .iconst(cl_type(&Type::Integer(bits))?, i64::from(variant));
        self.builder
            .ins()
            .store(MemFlagsData::new(), tag, pointer, 0);
        Ok(())
    }

    pub(super) fn enum_switch(
        &mut self,
        value: &Name,
        cases: &[(u32, BlockTarget)],
        default: Option<&BlockTarget>,
    ) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(value)?;
        let tag = self.enum_tag_value(value, bits)?;
        let tag_ty = cl_type(&Type::Integer(bits))?;
        for (variant, target) in cases {
            let key = self.builder.ins().iconst(tag_ty, i64::from(*variant));
            let equal = self.builder.ins().icmp(IntCC::Equal, tag, key);
            let hit = self.blocks[&target.block];
            let miss = self.builder.create_block();
            let args = self.edge_args(target)?;
            self.builder.ins().brif(equal, hit, &args, miss, &[]);
            self.builder.switch_to_block(miss);
        }
        if let Some(default) = default {
            let block = self.blocks[&default.block];
            let args = self.edge_args(default)?;
            self.builder.ins().jump(block, &args);
        } else {
            self.trap(1);
        }
        Ok(())
    }

    fn enum_tag_value(&mut self, value: &Name, bits: u16) -> Result<Value, EmitError> {
        let pointer = self.value(value)?;
        let ty = cl_type(&Type::Integer(bits))?;
        Ok(self.builder.ins().load(ty, MemFlagsData::new(), pointer, 0))
    }

    fn enum_info(&self, value: &Name) -> Result<(&str, u16), EmitError> {
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
            .layouts
            .declaration(name.as_str())
            .filter(|declaration| !declaration.variants.is_empty())
            .ok_or_else(|| err("unknown enum declaration"))?;
        Ok((
            declaration.name.as_str(),
            klf_layout::enum_declaration_tag_bits(declaration),
        ))
    }
}
