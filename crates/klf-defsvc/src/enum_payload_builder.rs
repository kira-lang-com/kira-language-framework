use super::struct_copy::{copy_struct_fields, copyable_struct, struct_copy_plan_with_variants};
use super::{BuilderValue, DefinitionServiceState};
use klf_kir::Type;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant_struct",
        state,
        DefinitionServiceState::type_enum_variant_struct,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new_struct",
        state,
        DefinitionServiceState::value_enum_new_struct,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_payload_struct",
        state,
        DefinitionServiceState::value_enum_payload_struct,
    );
}

impl DefinitionServiceState {
    fn type_enum_variant_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(enum_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(2), arguments.get(3)) else {
            return u64::MAX;
        };
        let Ok(enum_index) = usize::try_from(enum_id) else {
            return u64::MAX;
        };
        let Ok(struct_index) = usize::try_from(struct_id) else {
            return u64::MAX;
        };
        let Some((enum_module, enum_declaration)) = self.types.get(enum_index).cloned() else {
            return u64::MAX;
        };
        let Some((struct_module, struct_declaration)) = self.types.get(struct_index).cloned()
        else {
            return u64::MAX;
        };
        if enum_module != struct_module
            || !enum_declaration.fields.is_empty()
            || !struct_declaration.variants.is_empty()
            || !copyable_struct(&self.types, struct_module, &struct_declaration)
            || enum_declaration
                .variants
                .iter()
                .any(|variant| variant.name == name)
        {
            return u64::MAX;
        }
        let Some((_, declaration)) = self.types.get_mut(enum_index) else {
            return u64::MAX;
        };
        declaration.variants.push(klf_kir::EnumVariant {
            name,
            payload: vec![Type::Nominal(struct_declaration.name, Vec::new())],
        });
        0
    }

    fn value_enum_new_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(enum_type_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(variant) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| u32::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Some(struct_value_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(struct_value) = self.value_name(function_id, Some(struct_value_id)) else {
            return u64::MAX;
        };
        let Some(struct_index) = self
            .struct_values
            .iter()
            .position(|(id, _, _)| *id == struct_value_id)
        else {
            return u64::MAX;
        };
        let source_type_id = self.struct_values[struct_index].1;
        let source_initialized = self.struct_values[struct_index].2.clone();
        let Ok(enum_index) = usize::try_from(enum_type_id) else {
            return u64::MAX;
        };
        let Ok(source_type_index) = usize::try_from(source_type_id) else {
            return u64::MAX;
        };
        let Some((enum_module, enum_declaration)) = self.types.get(enum_index).cloned() else {
            return u64::MAX;
        };
        let Some((source_module, source_declaration)) = self.types.get(source_type_index).cloned()
        else {
            return u64::MAX;
        };
        let Some(enum_variant) = enum_declaration.variants.get(variant as usize) else {
            return u64::MAX;
        };
        if enum_module != source_module
            || source_initialized.len() != source_declaration.fields.len()
            || !source_initialized.iter().all(|initialized| *initialized)
            || enum_variant.payload
                != vec![Type::Nominal(source_declaration.name.clone(), Vec::new())]
        {
            return u64::MAX;
        }
        let variants = self.known_enum_variants(struct_value_id);
        let Some(plan) = struct_copy_plan_with_variants(
            &self.types,
            source_module,
            &source_declaration,
            &variants,
        ) else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.module != enum_module || function.builder.is_terminated() {
            return u64::MAX;
        }
        let address = function
            .builder
            .stack_alloc(Type::Nominal(enum_declaration.name, Vec::new()));
        let payload_address = function.builder.enum_payload_addr(address.clone(), variant);
        copy_struct_fields(
            &mut function.builder,
            &plan,
            &struct_value,
            &payload_address,
        );
        function.builder.enum_inject(address.clone(), variant);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: address,
        }));
        self.enum_values.push((id, enum_type_id));
        self.enum_variants.push((id, variant));
        self.enum_struct_payloads.push((id, struct_value_id));
        id
    }

    fn value_enum_payload_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(variant) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| u32::try_from(value).ok())
        else {
            return u64::MAX;
        };
        if let Some(actual_variant) = self
            .enum_variants
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, variant)| *variant)
            && actual_variant != variant
        {
            return u64::MAX;
        }
        let Some(enum_type_id) = self
            .enum_values
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, type_id)| *type_id)
        else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Ok(enum_type_index) = usize::try_from(enum_type_id) else {
            return u64::MAX;
        };
        let Some((module, enum_declaration)) = self.types.get(enum_type_index).cloned() else {
            return u64::MAX;
        };
        let Some(enum_variant) = enum_declaration.variants.get(variant as usize) else {
            return u64::MAX;
        };
        let Some(Type::Nominal(struct_name, arguments)) = enum_variant.payload.first() else {
            return u64::MAX;
        };
        if !arguments.is_empty() || enum_variant.payload.len() != 1 {
            return u64::MAX;
        }
        let Some((struct_type_id, struct_declaration)) = self
            .types
            .find(module, struct_name.as_str())
            .and_then(|index| Some((u64::try_from(index).ok()?, self.types[index].1.clone())))
        else {
            return u64::MAX;
        };
        if !copyable_struct(&self.types, module, &struct_declaration) {
            return u64::MAX;
        }
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.module != module || function.builder.is_terminated() {
            return u64::MAX;
        }
        let address = function.builder.enum_payload_addr(value, variant);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: address,
        }));
        self.struct_values.push((
            id,
            struct_type_id,
            vec![true; struct_declaration.fields.len()],
        ));
        for (path, nested_variant) in self.known_enum_variants(value_id) {
            self.struct_known_enum_variants
                .push((id, path, nested_variant));
        }
        id
    }
}
