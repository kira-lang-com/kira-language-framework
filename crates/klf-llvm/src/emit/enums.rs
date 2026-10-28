use super::FnEmitter;
use crate::EmitError;
use crate::lltype::llvm_type;
use klf_core::Name;
use klf_kir::{BlockTarget, Type};

impl FnEmitter<'_, '_> {
    pub(super) fn enum_tag(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(value)?;
        let operand = self.enum_tag_operand(value, bits)?;
        let result = super::single(results)?.clone();
        self.record(&result, operand);
        Ok(())
    }

    pub(super) fn enum_payload_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        variant: u32,
    ) -> Result<(), EmitError> {
        let (name, _) = self.enum_info(base)?;
        let offset = klf_layout::enum_payload_offset(name, variant, &self.module.types)
            .ok_or_else(|| EmitError("enum variant has no payload area".to_owned()))?;
        let base_value = self.value(base)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = getelementptr inbounds i8, ptr {base_value}, i64 {offset}"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn enum_inject(&mut self, address: &Name, variant: u32) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(address)?;
        let pointer = self.value(address)?;
        self.line(&format!(
            "  call void @klfrt_store(ptr {pointer}, i32 {}, i64 {})",
            bits / 8,
            variant
        ));
        Ok(())
    }

    pub(super) fn enum_switch(
        &mut self,
        value: &Name,
        cases: &[(u32, BlockTarget)],
        default: Option<&BlockTarget>,
    ) -> Result<(), EmitError> {
        let (_, bits) = self.enum_info(value)?;
        let tag = self.enum_tag_operand(value, bits)?;
        let tag_type = llvm_type(&Type::Integer(bits))?;
        let default_prep = default.map(|_| self.fresh_label("prep.enum.default"));
        let unreachable = default
            .is_none()
            .then(|| self.fresh_label("enum.unreachable"));
        let mut arms = Vec::new();
        let mut preps = Vec::new();
        for (variant, target) in cases {
            let prep = self.fresh_label("prep.enum.case");
            arms.push(format!("{tag_type} {variant}, label %{prep}"));
            preps.push((prep, target));
        }
        let fallback = default_prep
            .as_deref()
            .or(unreachable.as_deref())
            .expect("enum switch has a fallback");
        self.line(&format!(
            "  switch {tag_type} {tag}, label %{fallback} [ {} ]",
            arms.join(" ")
        ));
        for (prep, target) in preps {
            self.line(&format!("{prep}:"));
            self.store_edge(target)?;
            self.line(&format!("  br label %\"{}\"", target.block.as_str()));
        }
        if let Some(default) = default {
            let prep = default_prep.expect("default prep exists");
            self.line(&format!("{prep}:"));
            self.store_edge(default)?;
            self.line(&format!("  br label %\"{}\"", default.block.as_str()));
        } else {
            let unreachable = unreachable.expect("unreachable label exists");
            self.line(&format!("{unreachable}:"));
            self.trap(1);
        }
        Ok(())
    }

    fn enum_tag_operand(&mut self, value: &Name, bits: u16) -> Result<String, EmitError> {
        let pointer = self.value(value)?;
        let raw = self.fresh();
        self.line(&format!(
            "  {raw} = call i64 @klfrt_load(ptr {pointer}, i32 {})",
            bits / 8
        ));
        let tag = self.fresh();
        self.line(&format!("  {tag} = trunc i64 {raw} to i{bits}"));
        Ok(tag)
    }

    fn enum_info(&self, value: &Name) -> Result<(&str, u16), EmitError> {
        let ty = self.types.require(value)?;
        let (Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner)) = ty else {
            return Err(EmitError(
                "enum operation requires an enum address".to_owned(),
            ));
        };
        let Type::Nominal(name, _) = inner.as_ref() else {
            return Err(EmitError(
                "enum operation requires a nominal enum".to_owned(),
            ));
        };
        let declaration = self
            .module
            .types
            .iter()
            .find(|declaration| declaration.name == *name)
            .filter(|declaration| !declaration.variants.is_empty())
            .ok_or_else(|| EmitError("unknown enum declaration".to_owned()))?;
        Ok((
            declaration.name.as_str(),
            klf_layout::enum_declaration_tag_bits(declaration),
        ))
    }
}
