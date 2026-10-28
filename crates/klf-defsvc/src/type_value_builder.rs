//! The value half of the struct and enum builder services: constructing a
//! value of a declared aggregate type and reading or writing its parts.

use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{ConvertKind, StoreMode, Type};

impl DefinitionServiceState {
    pub(super) fn value_enum_new(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_new_typed(arguments, None)
    }

    pub(super) fn value_enum_new_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_new_typed(arguments, Some(&Type::Integer(64)))
    }

    pub(super) fn value_enum_new_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_new_typed(arguments, Some(&Type::Integer(32)))
    }

    pub(super) fn value_enum_new_typed(
        &mut self,
        arguments: &[u64],
        payload_type: Option<&Type>,
    ) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(type_id) = arguments.get(1).copied() else {
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
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((module, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        let Some(enum_variant) = declaration.variants.get(variant as usize) else {
            return u64::MAX;
        };
        let expected_payload = payload_type.map_or_else(Vec::new, |ty| vec![ty.clone()]);
        if enum_variant.payload != expected_payload {
            return u64::MAX;
        }
        let module = *module;
        let enum_name = declaration.name.clone();
        let payload = match (payload_type, arguments.get(3).copied()) {
            (Some(expected), Some(payload_id)) => {
                let Some(payload_name) = self.value_name(function_id, Some(payload_id)) else {
                    return u64::MAX;
                };
                if self
                    .arrays
                    .iter()
                    .any(|(id, array)| *id == payload_id && array.function == function_id)
                    || self.enum_values.iter().any(|(id, _)| *id == payload_id)
                    || self.bool_values.contains(&payload_id)
                    || (expected == &Type::Integer(32)) != self.i32_values.contains(&payload_id)
                {
                    return u64::MAX;
                }
                Some(payload_name)
            }
            (Some(_), None) | (None, Some(_)) => return u64::MAX,
            (None, None) => None,
        };
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
            .stack_alloc(Type::Nominal(enum_name, Vec::new()));
        if let Some(payload) = payload {
            let payload_address = function.builder.enum_payload_addr(address.clone(), variant);
            function
                .builder
                .store(StoreMode::Init, payload, payload_address);
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
        self.enum_values.push((id, type_id));
        self.enum_variants.push((id, variant));
        id
    }

    pub(super) fn value_struct_new(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(type_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((module, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        if !declaration.variants.is_empty() {
            return u64::MAX;
        }
        let module = *module;
        let name = declaration.name.clone();
        let field_count = declaration.fields.len();
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
            .stack_alloc(Type::Nominal(name, Vec::new()));
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: address,
        }));
        self.struct_values
            .push((id, type_id, vec![false; field_count]));
        id
    }

    pub(super) fn value_struct_set_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_set(arguments, &Type::Integer(64))
    }

    pub(super) fn value_struct_set_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_set(arguments, &Type::Integer(32))
    }

    pub(super) fn value_struct_set(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Some(payload_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(base) = self.value_name(function_id, Some(struct_id)) else {
            return u64::MAX;
        };
        let Some(payload) = self.value_name(function_id, Some(payload_id)) else {
            return u64::MAX;
        };
        let Some(struct_index) = self
            .struct_values
            .iter()
            .position(|(id, _, _)| *id == struct_id)
        else {
            return u64::MAX;
        };
        let type_id = self.struct_values[struct_index].1;
        if self.struct_values[struct_index].2.get(field).copied() != Some(false) {
            return u64::MAX;
        }
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
        if field_type != *element
            || self
                .arrays
                .iter()
                .any(|(id, array)| *id == payload_id && array.function == function_id)
            || self.enum_values.iter().any(|(id, _)| *id == payload_id)
            || self
                .struct_values
                .iter()
                .any(|(id, _, _)| *id == payload_id)
            || self.bool_values.contains(&payload_id)
            || (*element == Type::Integer(32)) != self.i32_values.contains(&payload_id)
        {
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
        let address = function
            .builder
            .field_addr(base, u32::try_from(field).unwrap_or(u32::MAX));
        function.builder.store(StoreMode::Init, payload, address);
        self.struct_values[struct_index].2[field] = true;
        0
    }

    pub(super) fn value_struct_get_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_get(arguments, &Type::Integer(64))
    }

    pub(super) fn value_struct_get_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_struct_get(arguments, &Type::Integer(32))
    }

    pub(super) fn value_struct_get(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(struct_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(field) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
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
        if field_type != *element {
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
        let address = function
            .builder
            .field_addr(base, u32::try_from(field).unwrap_or(u32::MAX));
        let name = function.builder.load(address);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *element == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    pub(super) fn value_enum_payload_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_payload(arguments, &Type::Integer(64))
    }

    pub(super) fn value_enum_payload_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_payload(arguments, &Type::Integer(32))
    }

    pub(super) fn value_enum_payload(&mut self, arguments: &[u64], element: &Type) -> u64 {
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
        let Some(type_id) = self
            .enum_values
            .iter()
            .find(|(enum_value, _)| *enum_value == value_id)
            .map(|(_, type_id)| *type_id)
        else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((_, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        let Some(enum_variant) = declaration.variants.get(variant as usize) else {
            return u64::MAX;
        };
        if enum_variant.payload != vec![element.clone()] {
            return u64::MAX;
        }
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let address = function.builder.enum_payload_addr(value, variant);
        let name = function.builder.load(address);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *element == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    pub(super) fn value_enum_tag_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_tag(arguments, &Type::Integer(64))
    }

    pub(super) fn value_enum_tag_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_enum_tag(arguments, &Type::Integer(32))
    }

    pub(super) fn value_enum_tag(&mut self, arguments: &[u64], result_type: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        if self
            .enum_values
            .iter()
            .all(|(enum_value, _)| *enum_value != value_id)
        {
            return u64::MAX;
        }
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let tag = function.builder.enum_tag(value);
        let name = function
            .builder
            .convert(ConvertKind::ZeroExtend, tag, result_type.clone());
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *result_type == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }
}
