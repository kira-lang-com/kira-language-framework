use super::DefinitionServiceState;
use klf_kir::{EnumVariant, Field, LayoutRule, Policy, Type, TypeDeclaration};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_type_enum_new",
        state,
        DefinitionServiceState::type_enum_new,
    );
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant",
        state,
        DefinitionServiceState::type_enum_variant,
    );
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant_i64",
        state,
        DefinitionServiceState::type_enum_variant_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant_i32",
        state,
        DefinitionServiceState::type_enum_variant_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_type_struct_new",
        state,
        DefinitionServiceState::type_struct_new,
    );
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_i64",
        state,
        DefinitionServiceState::type_struct_field_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_i32",
        state,
        DefinitionServiceState::type_struct_field_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new",
        state,
        DefinitionServiceState::value_enum_new,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new_i64",
        state,
        DefinitionServiceState::value_enum_new_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new_i32",
        state,
        DefinitionServiceState::value_enum_new_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_payload_i64",
        state,
        DefinitionServiceState::value_enum_payload_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_payload_i32",
        state,
        DefinitionServiceState::value_enum_payload_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_new",
        state,
        DefinitionServiceState::value_struct_new,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_i64",
        state,
        DefinitionServiceState::value_struct_set_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_i32",
        state,
        DefinitionServiceState::value_struct_set_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_i64",
        state,
        DefinitionServiceState::value_struct_get_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_i32",
        state,
        DefinitionServiceState::value_struct_get_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_tag_i64",
        state,
        DefinitionServiceState::value_enum_tag_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_tag_i32",
        state,
        DefinitionServiceState::value_enum_tag_i32,
    );
}

impl DefinitionServiceState {
    fn type_enum_new(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(module_index) = usize::try_from(module) else {
            return u64::MAX;
        };
        if self
            .modules
            .get(module_index)
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let id = u64::try_from(self.types.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.types.push(
            module,
            TypeDeclaration::empty(name, LayoutRule::Ridl, Policy::trivial(), span),
        );
        id
    }

    fn type_struct_new(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(module_index) = usize::try_from(module) else {
            return u64::MAX;
        };
        if self
            .modules
            .get(module_index)
            .and_then(Option::as_ref)
            .is_none()
        {
            return u64::MAX;
        }
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let id = u64::try_from(self.types.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.types.push(
            module,
            TypeDeclaration::empty(name, LayoutRule::Ridl, Policy::trivial(), span),
        );
        id
    }

    fn type_struct_field_i64(&mut self, arguments: &[u64]) -> u64 {
        self.type_struct_field(arguments, Type::Integer(64))
    }

    fn type_struct_field_i32(&mut self, arguments: &[u64]) -> u64 {
        self.type_struct_field(arguments, Type::Integer(32))
    }

    fn type_struct_field(&mut self, arguments: &[u64], ty: Type) -> u64 {
        let Some(type_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(type_index) else {
            return u64::MAX;
        };
        if declaration.fields.iter().any(|field| field.name == name) {
            return u64::MAX;
        }
        declaration.fields.push(Field::new(name, ty));
        0
    }

    fn type_enum_variant(&mut self, arguments: &[u64]) -> u64 {
        self.type_enum_variant_typed(arguments, None)
    }

    fn type_enum_variant_i64(&mut self, arguments: &[u64]) -> u64 {
        self.type_enum_variant_typed(arguments, Some(Type::Integer(64)))
    }

    fn type_enum_variant_i32(&mut self, arguments: &[u64]) -> u64 {
        self.type_enum_variant_typed(arguments, Some(Type::Integer(32)))
    }

    fn type_enum_variant_typed(&mut self, arguments: &[u64], payload: Option<Type>) -> u64 {
        let Some(type_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(type_index) else {
            return u64::MAX;
        };
        if declaration
            .variants
            .iter()
            .any(|variant| variant.name == name)
        {
            return u64::MAX;
        }
        declaration.variants.push(EnumVariant {
            name,
            payload: payload.into_iter().collect(),
        });
        0
    }
}
