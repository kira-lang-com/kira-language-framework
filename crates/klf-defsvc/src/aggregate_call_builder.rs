use super::{BuilderValue, DefinitionServiceState};
use klf_core::Name;
use klf_kir::Type;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_new_struct_i64",
        state,
        DefinitionServiceState::function_new_struct_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_enum_i64",
        state,
        DefinitionServiceState::function_new_enum_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_array_i64",
        state,
        DefinitionServiceState::function_new_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_array_i32",
        state,
        DefinitionServiceState::function_new_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_struct",
        state,
        DefinitionServiceState::function_param_struct,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_enum",
        state,
        DefinitionServiceState::function_param_enum,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_array_i64",
        state,
        DefinitionServiceState::function_param_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_array_i32",
        state,
        DefinitionServiceState::function_param_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_struct_i64",
        state,
        DefinitionServiceState::function_call_struct_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_enum_i64",
        state,
        DefinitionServiceState::function_call_enum_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_array_i64",
        state,
        DefinitionServiceState::function_call_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_array_i32",
        state,
        DefinitionServiceState::function_call_array_i32,
    );
}

impl DefinitionServiceState {
    fn function_new_struct_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(type_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(2), arguments.get(3)) else {
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
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((owner, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        if *owner != module || !declaration.variants.is_empty() {
            return u64::MAX;
        }
        let parameter_type = struct_type(&declaration.name);
        let parameters = vec![(Name::new("arg0"), parameter_type.clone())];
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &parameters, Type::Integer(64), span),
            result_type: Type::Integer(64),
            parameters: vec![Name::new("arg0")],
            parameter_types: vec![parameter_type],
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_param_struct(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(1)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some((module, name, parameter_type)) = self
            .functions
            .get(function_index)
            .and_then(Option::as_ref)
            .filter(|function| !function.builder.is_terminated())
            .and_then(|function| {
                Some((
                    function.module,
                    function.parameters.get(index)?.clone(),
                    function.parameter_types.get(index)?.clone(),
                ))
            })
        else {
            return u64::MAX;
        };
        let Type::Address(inner) = parameter_type else {
            return u64::MAX;
        };
        let Type::Nominal(struct_name, arguments) = *inner else {
            return u64::MAX;
        };
        if !arguments.is_empty() {
            return u64::MAX;
        }
        let Some(type_index) = self
            .types
            .find(module, struct_name.as_str())
            .filter(|index| self.types[*index].1.variants.is_empty())
        else {
            return u64::MAX;
        };
        let declaration = &self.types[type_index].1;
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        self.struct_values.push((
            id,
            u64::try_from(type_index).unwrap_or(u64::MAX),
            vec![true; declaration.fields.len()],
        ));
        id
    }

    fn function_new_enum_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(type_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(2), arguments.get(3)) else {
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
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some((owner, declaration)) = self.types.get(type_index) else {
            return u64::MAX;
        };
        if *owner != module || declaration.variants.is_empty() {
            return u64::MAX;
        }
        let parameter_type = enum_type(&declaration.name);
        let parameters = vec![(Name::new("arg0"), parameter_type.clone())];
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &parameters, Type::Integer(64), span),
            result_type: Type::Integer(64),
            parameters: vec![Name::new("arg0")],
            parameter_types: vec![parameter_type],
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_new_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_array(arguments, Type::Integer(64))
    }

    fn function_new_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_array(arguments, Type::Integer(32))
    }

    fn function_new_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(module) = arguments.first().copied() else {
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
        let Some((name, span)) = self.source_name(arguments.get(2), arguments.get(3)) else {
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
        let Ok(length) = u64::try_from(length) else {
            return u64::MAX;
        };
        let parameter_type = array_type(length, element);
        let parameters = vec![(Name::new("arg0"), parameter_type.clone())];
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &parameters, Type::Integer(64), span),
            result_type: Type::Integer(64),
            parameters: vec![Name::new("arg0")],
            parameter_types: vec![parameter_type],
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_param_enum(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(1)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some((module, name, parameter_type)) = self
            .functions
            .get(function_index)
            .and_then(Option::as_ref)
            .filter(|function| !function.builder.is_terminated())
            .and_then(|function| {
                Some((
                    function.module,
                    function.parameters.get(index)?.clone(),
                    function.parameter_types.get(index)?.clone(),
                ))
            })
        else {
            return u64::MAX;
        };
        let Type::Address(inner) = parameter_type else {
            return u64::MAX;
        };
        let Type::Nominal(enum_name, arguments) = *inner else {
            return u64::MAX;
        };
        if !arguments.is_empty() {
            return u64::MAX;
        }
        let Some(type_index) = self
            .types
            .find(module, enum_name.as_str())
            .filter(|index| !self.types[*index].1.variants.is_empty())
        else {
            return u64::MAX;
        };
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        self.enum_values
            .push((id, u64::try_from(type_index).unwrap_or(u64::MAX)));
        id
    }

    fn function_param_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_param_array(arguments, Type::Integer(64))
    }

    fn function_param_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_param_array(arguments, Type::Integer(32))
    }

    fn function_param_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(1)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some((name, parameter_type)) = self
            .functions
            .get(function_index)
            .and_then(Option::as_ref)
            .filter(|function| !function.builder.is_terminated())
            .and_then(|function| {
                Some((
                    function.parameters.get(index)?.clone(),
                    function.parameter_types.get(index)?.clone(),
                ))
            })
        else {
            return u64::MAX;
        };
        let Type::Address(inner) = parameter_type else {
            return u64::MAX;
        };
        let Type::Array(length, inner_element) = *inner else {
            return u64::MAX;
        };
        if *inner_element != element {
            return u64::MAX;
        }
        let Ok(length) = usize::try_from(length) else {
            return u64::MAX;
        };
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

    fn function_call_struct_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let Ok(host_index) = usize::try_from(host_id) else {
            return u64::MAX;
        };
        let Ok(callee_index) = usize::try_from(callee_id) else {
            return u64::MAX;
        };
        let Some((struct_id, initialized)) = self
            .struct_values
            .iter()
            .find(|(id, _, _)| *id == value_id)
            .map(|(_, type_id, initialized)| (*type_id, initialized.clone()))
        else {
            return u64::MAX;
        };
        if initialized.iter().any(|initialized| !initialized) {
            return u64::MAX;
        }
        let Ok(struct_index) = usize::try_from(struct_id) else {
            return u64::MAX;
        };
        let Some(parameter_type) = self
            .types
            .get(struct_index)
            .map(|(_, declaration)| struct_type(&declaration.name))
        else {
            return u64::MAX;
        };
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(&parameter_type)
            || callee.result_type != Type::Integer(64)
        {
            return u64::MAX;
        }
        let Some(value) = self.value_name(host_id, Some(value_id)) else {
            return u64::MAX;
        };
        let callee_name = callee.reference();
        let callee_module = callee.module;
        let Some(Some(host)) = self.functions.get_mut(host_index) else {
            return u64::MAX;
        };
        if host.module != callee_module || host.builder.is_terminated() {
            return u64::MAX;
        }
        let name = host.builder.call(callee_name, vec![value]);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: host_id,
            name,
        }));
        id
    }

    fn function_call_enum_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let Ok(host_index) = usize::try_from(host_id) else {
            return u64::MAX;
        };
        let Ok(callee_index) = usize::try_from(callee_id) else {
            return u64::MAX;
        };
        let Some(type_id) = self
            .enum_values
            .iter()
            .find(|(id, _)| *id == value_id)
            .map(|(_, type_id)| *type_id)
        else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        let Some(parameter_type) = self
            .types
            .get(type_index)
            .map(|(_, declaration)| enum_type(&declaration.name))
        else {
            return u64::MAX;
        };
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(&parameter_type)
            || callee.result_type != Type::Integer(64)
        {
            return u64::MAX;
        }
        let Some(value) = self.value_name(host_id, Some(value_id)) else {
            return u64::MAX;
        };
        let callee_name = callee.reference();
        let callee_module = callee.module;
        let Some(Some(host)) = self.functions.get_mut(host_index) else {
            return u64::MAX;
        };
        if host.module != callee_module || host.builder.is_terminated() {
            return u64::MAX;
        }
        let name = host.builder.call(callee_name, vec![value]);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: host_id,
            name,
        }));
        id
    }

    fn function_call_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_array(arguments, Type::Integer(64))
    }

    fn function_call_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_array(arguments, Type::Integer(32))
    }

    fn function_call_array(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let Ok(host_index) = usize::try_from(host_id) else {
            return u64::MAX;
        };
        let Ok(callee_index) = usize::try_from(callee_id) else {
            return u64::MAX;
        };
        let Some((array_length, initialized)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == value_id && array.function == host_id)
            .map(|(_, array)| (array.length, array.initialized.clone()))
        else {
            return u64::MAX;
        };
        if !initialized.iter().all(|initialized| *initialized) {
            return u64::MAX;
        }
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        let parameter_type = array_type(u64::try_from(array_length).unwrap_or(u64::MAX), element);
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(&parameter_type)
            || callee.result_type != Type::Integer(64)
        {
            return u64::MAX;
        }
        let Some(value) = self.value_name(host_id, Some(value_id)) else {
            return u64::MAX;
        };
        let callee_name = callee.reference();
        let callee_module = callee.module;
        let Some(Some(host)) = self.functions.get_mut(host_index) else {
            return u64::MAX;
        };
        if host.module != callee_module || host.builder.is_terminated() {
            return u64::MAX;
        }
        let name = host.builder.call(callee_name, vec![value]);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: host_id,
            name,
        }));
        id
    }
}

fn struct_type(name: &Name) -> Type {
    Type::Address(Box::new(Type::Nominal(name.clone(), Vec::new())))
}

fn enum_type(name: &Name) -> Type {
    Type::Address(Box::new(Type::Nominal(name.clone(), Vec::new())))
}

fn array_type(length: u64, element: Type) -> Type {
    Type::Address(Box::new(Type::Array(length, Box::new(element))))
}
