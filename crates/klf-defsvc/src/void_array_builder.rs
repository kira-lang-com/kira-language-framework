use super::DefinitionServiceState;
use klf_core::Name;
use klf_kir::Type;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_new_void_array_i64",
        state,
        DefinitionServiceState::function_new_void_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_void_array_i32",
        state,
        DefinitionServiceState::function_new_void_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void_array_i64",
        state,
        DefinitionServiceState::function_call_void_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void_array_i32",
        state,
        DefinitionServiceState::function_call_void_array_i32,
    );
}

impl DefinitionServiceState {
    fn function_new_void_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_void_array(arguments, Type::Integer(64))
    }

    fn function_new_void_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_void_array(arguments, Type::Integer(32))
    }

    fn function_new_void_array(&mut self, arguments: &[u64], element: Type) -> u64 {
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
            builder: klf_services::FunctionBuilder::new(name, &parameters, Type::Unit, span),
            result_type: Type::Unit,
            parameters: vec![Name::new("arg0")],
            parameter_types: vec![parameter_type],
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_call_void_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void_array(arguments, Type::Integer(64))
    }

    fn function_call_void_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void_array(arguments, Type::Integer(32))
    }

    fn function_call_void_array(&mut self, arguments: &[u64], element: Type) -> u64 {
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
        let Some((array_length, array_element, initialized)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == value_id && array.function == host_id)
            .map(|(_, array)| {
                (
                    array.length,
                    array.element.clone(),
                    array.initialized.clone(),
                )
            })
        else {
            return u64::MAX;
        };
        if array_element != element || initialized.iter().any(|initialized| !initialized) {
            return u64::MAX;
        }
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        let parameter_type = array_type(u64::try_from(array_length).unwrap_or(u64::MAX), element);
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(&parameter_type)
            || callee.result_type != Type::Unit
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
        host.builder.call_void(callee_name, vec![value]);
        0
    }
}

fn array_type(length: u64, element: Type) -> Type {
    Type::Address(Box::new(Type::Array(length, Box::new(element))))
}
