use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{StoreMode, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant_array_i64",
        state,
        DefinitionServiceState::type_enum_variant_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_type_enum_variant_array_i32",
        state,
        DefinitionServiceState::type_enum_variant_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new_array_i64",
        state,
        DefinitionServiceState::value_enum_new_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_new_array_i32",
        state,
        DefinitionServiceState::value_enum_new_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_payload_array_i64",
        state,
        DefinitionServiceState::value_enum_payload_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_enum_payload_array_i32",
        state,
        DefinitionServiceState::value_enum_payload_array_i32,
    );
}

impl DefinitionServiceState {
    fn type_enum_variant_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.type_enum_variant_array(arguments, Type::Integer(64))
    }

    fn type_enum_variant_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.type_enum_variant_array(arguments, Type::Integer(32))
    }

    fn type_enum_variant_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(enum_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(length) = arguments
            .get(1)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Some((name, _)) = self.source_name(arguments.get(2), arguments.get(3)) else {
            return u64::MAX;
        };
        let Ok(enum_index) = usize::try_from(enum_id) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get(enum_index) else {
            return u64::MAX;
        };
        if declaration.fields.is_empty()
            && declaration
                .variants
                .iter()
                .any(|variant| variant.name == name)
        {
            return u64::MAX;
        }
        let Ok(length) = u64::try_from(length) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(enum_index) else {
            return u64::MAX;
        };
        if declaration
            .variants
            .iter()
            .any(|variant| variant.name == name)
        {
            return u64::MAX;
        }
        declaration.variants.push(klf_kir::EnumVariant {
            name,
            payload: vec![Type::Array(length, Box::new(element))],
        });
        0
    }

    fn value_enum_new_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_new_array(arguments, &Type::Integer(64))
    }

    fn value_enum_new_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_new_array(arguments, &Type::Integer(32))
    }

    fn value_enum_new_array(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(enum_type_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(variant) = variant_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(array_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        if self.value_name(function_id, Some(array_id)).is_none() {
            return u64::MAX;
        }
        let Some((array_name, array_element, array_length, initialized)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == array_id && array.function == function_id)
            .map(|(_, array)| {
                (
                    array.name.clone(),
                    array.element.clone(),
                    array.length,
                    array.initialized.clone(),
                )
            })
        else {
            return u64::MAX;
        };
        if array_element != *element || !initialized.iter().all(|initialized| *initialized) {
            return u64::MAX;
        }
        let Ok(enum_type_index) = usize::try_from(enum_type_id) else {
            return u64::MAX;
        };
        let Some((module, enum_declaration)) = self.types.get(enum_type_index).cloned() else {
            return u64::MAX;
        };
        let Some(enum_variant) = enum_declaration.variants.get(variant as usize) else {
            return u64::MAX;
        };
        if enum_variant.payload
            != vec![Type::Array(
                u64::try_from(array_length).unwrap_or(u64::MAX),
                Box::new(element.clone()),
            )]
        {
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
        let address = function
            .builder
            .stack_alloc(Type::Nominal(enum_declaration.name, Vec::new()));
        let payload_address = function.builder.enum_payload_addr(address.clone(), variant);
        for index in 0..array_length {
            let index_name = function
                .builder
                .const_int(Type::Integer(64), i128::try_from(index).unwrap_or(0));
            let source_field = function
                .builder
                .elem_addr(array_name.clone(), index_name.clone());
            let value = function.builder.load(source_field);
            let destination_field = function
                .builder
                .elem_addr(payload_address.clone(), index_name);
            function
                .builder
                .store(StoreMode::Init, value, destination_field);
        }
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
        id
    }

    fn value_enum_payload_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_payload_array(arguments, Type::Integer(64))
    }

    fn value_enum_payload_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_payload_array(arguments, Type::Integer(32))
    }

    fn value_enum_payload_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(variant) = variant_index(arguments.get(2)) else {
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
        let Some(Type::Array(length, field_element)) = enum_variant.payload.first() else {
            return u64::MAX;
        };
        let Ok(length) = usize::try_from(*length) else {
            return u64::MAX;
        };
        if enum_variant.payload.len() != 1 || **field_element != element {
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
        let name = function.builder.enum_payload_addr(value, variant);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: name.clone(),
        }));
        self.arrays.push((
            id,
            super::aggregate_builder::BuilderArray {
                function: function_id,
                name,
                element,
                length,
                initialized: vec![true; length],
            },
        ));
        id
    }
}

fn variant_index(value: Option<&u64>) -> Option<u32> {
    value
        .copied()
        .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
        .and_then(|value| u32::try_from(value).ok())
}
