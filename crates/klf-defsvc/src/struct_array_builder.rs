use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{StoreMode, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_array_i64",
        state,
        DefinitionServiceState::type_struct_field_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_type_struct_field_array_i32",
        state,
        DefinitionServiceState::type_struct_field_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_array_i64",
        state,
        DefinitionServiceState::value_struct_set_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_set_array_i32",
        state,
        DefinitionServiceState::value_struct_set_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_array_i64",
        state,
        DefinitionServiceState::value_struct_get_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_struct_get_array_i32",
        state,
        DefinitionServiceState::value_struct_get_array_i32,
    );
}

impl DefinitionServiceState {
    fn type_struct_field_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.type_struct_field_array(arguments, Type::Integer(64))
    }

    fn type_struct_field_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.type_struct_field_array(arguments, Type::Integer(32))
    }

    fn type_struct_field_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(struct_id) = arguments.first().copied() else {
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
        let Ok(struct_index) = usize::try_from(struct_id) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get(struct_index) else {
            return u64::MAX;
        };
        if !declaration.variants.is_empty()
            || declaration.fields.iter().any(|field| field.name == name)
        {
            return u64::MAX;
        }
        let Ok(length) = u64::try_from(length) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get_mut(struct_index) else {
            return u64::MAX;
        };
        declaration.fields.push(klf_kir::Field::new(
            name,
            Type::Array(length, Box::new(element)),
        ));
        0
    }

    fn value_struct_set_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_set_array(arguments, &Type::Integer(64))
    }

    fn value_struct_set_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_set_array(arguments, &Type::Integer(32))
    }

    fn value_struct_set_array(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = field_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(array_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
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
        let Some(struct_index) = self
            .struct_values
            .iter()
            .position(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        if self.struct_values[struct_index].2.get(field).copied() != Some(false)
            || array_element != *element
            || !initialized.iter().all(|initialized| *initialized)
        {
            return u64::MAX;
        }
        let type_id = self.struct_values[struct_index].1;
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some(field_type) = self
            .types
            .get(type_index)
            .and_then(|(_, declaration)| declaration.fields.get(field))
            .map(|field| field.ty.clone())
        else {
            return u64::MAX;
        };
        let Type::Array(length, field_element) = field_type else {
            return u64::MAX;
        };
        let Ok(length) = usize::try_from(length) else {
            return u64::MAX;
        };
        if length != array_length || *field_element != *element {
            return u64::MAX;
        }
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let destination = function
            .builder
            .field_addr(base, u32::try_from(field).unwrap_or(u32::MAX));
        for index in 0..array_length {
            let index_name = function
                .builder
                .const_int(Type::Integer(64), i128::try_from(index).unwrap_or(0));
            let source_field = function
                .builder
                .elem_addr(array_name.clone(), index_name.clone());
            let value = function.builder.load(source_field);
            let destination_field = function.builder.elem_addr(destination.clone(), index_name);
            function
                .builder
                .store(StoreMode::Init, value, destination_field);
        }
        self.struct_values[struct_index].2[field] = true;
        0
    }

    fn value_struct_get_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_get_array(arguments, Type::Integer(64))
    }

    fn value_struct_get_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_get_array(arguments, Type::Integer(32))
    }

    fn value_struct_get_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = field_index(arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some((_, type_id, initialized)) = self
            .struct_values
            .iter()
            .find(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        if initialized.get(field).copied() != Some(true) {
            return u64::MAX;
        }
        let Ok(type_index) = usize::try_from(*type_id) else {
            return u64::MAX;
        };
        let Some(field_type) = self
            .types
            .get(type_index)
            .and_then(|(_, declaration)| declaration.fields.get(field))
            .map(|field| field.ty.clone())
        else {
            return u64::MAX;
        };
        let Type::Array(length, field_element) = field_type else {
            return u64::MAX;
        };
        let Ok(length) = usize::try_from(length) else {
            return u64::MAX;
        };
        if *field_element != element {
            return u64::MAX;
        }
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let name = function
            .builder
            .field_addr(base, u32::try_from(field).unwrap_or(u32::MAX));
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

fn field_index(value: Option<&u64>) -> Option<usize> {
    value
        .copied()
        .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
        .and_then(|value| usize::try_from(value).ok())
}
