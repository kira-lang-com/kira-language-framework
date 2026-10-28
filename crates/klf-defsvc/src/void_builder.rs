use super::DefinitionServiceState;
use klf_kir::Type;
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_new_void",
        state,
        DefinitionServiceState::function_new_void,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_void_i64",
        state,
        DefinitionServiceState::function_new_void_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_void_i32",
        state,
        DefinitionServiceState::function_new_void_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_return_void",
        state,
        DefinitionServiceState::function_return_void,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void",
        state,
        DefinitionServiceState::function_call_void,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void_i64",
        state,
        DefinitionServiceState::function_call_void_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void_i32",
        state,
        DefinitionServiceState::function_call_void_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void2_i64",
        state,
        DefinitionServiceState::function_call_void2_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_void2_i32",
        state,
        DefinitionServiceState::function_call_void2_i32,
    );
}

impl DefinitionServiceState {
    fn function_new_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
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
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &[], Type::Unit, span),
            result_type: Type::Unit,
            parameters: Vec::new(),
            parameter_types: Vec::new(),
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_new_void_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_void_typed(arguments, &Type::Integer(64))
    }

    fn function_new_void_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_void_typed(arguments, &Type::Integer(32))
    }

    fn function_new_void_typed(&mut self, arguments: &[u64], parameter_type: &Type) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((name, span)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(count) = arguments
            .get(3)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Ok(module_index) = usize::try_from(module) else {
            return u64::MAX;
        };
        if !(1..=2).contains(&count)
            || self
                .modules
                .get(module_index)
                .and_then(Option::as_ref)
                .is_none()
        {
            return u64::MAX;
        }
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        let parameters = (0..count)
            .map(|index| {
                (
                    klf_core::Name::new(format!("arg{index}")),
                    parameter_type.clone(),
                )
            })
            .collect::<Vec<_>>();
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(name, &parameters, Type::Unit, span),
            result_type: Type::Unit,
            parameters: parameters.iter().map(|(name, _)| name.clone()).collect(),
            parameter_types: parameters.into_iter().map(|(_, ty)| ty).collect(),
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_return_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.result_type != Type::Unit || function.builder.is_terminated() {
            return u64::MAX;
        }
        function.builder.ret(None);
        0
    }

    fn function_call_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Ok(host_index) = usize::try_from(host_id) else {
            return u64::MAX;
        };
        let Ok(callee_index) = usize::try_from(callee_id) else {
            return u64::MAX;
        };
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        if !callee.parameters.is_empty() || callee.result_type != Type::Unit {
            return u64::MAX;
        }
        let callee_name = callee.reference();
        let callee_module = callee.module;
        let Some(Some(host)) = self.functions.get_mut(host_index) else {
            return u64::MAX;
        };
        if host.module != callee_module || host.builder.is_terminated() {
            return u64::MAX;
        }
        host.builder.call_void(callee_name, Vec::new());
        0
    }

    fn function_call_void_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void_typed(arguments, &Type::Integer(64))
    }

    fn function_call_void_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void_typed(arguments, &Type::Integer(32))
    }

    fn function_call_void2_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void2_typed(arguments, &Type::Integer(64))
    }

    fn function_call_void2_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_void2_typed(arguments, &Type::Integer(32))
    }

    fn function_call_void2_typed(&mut self, arguments: &[u64], parameter_type: &Type) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(lhs_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let Some(rhs_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Ok(host_index) = usize::try_from(host_id) else {
            return u64::MAX;
        };
        let Ok(callee_index) = usize::try_from(callee_id) else {
            return u64::MAX;
        };
        let Some(lhs) = self.value_name(host_id, Some(lhs_id)) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(host_id, Some(rhs_id)) else {
            return u64::MAX;
        };
        let invalid_value = |value_id| {
            self.arrays
                .iter()
                .any(|(id, array)| *id == value_id && array.function == host_id)
                || self.enum_values.iter().any(|(id, _)| *id == value_id)
                || self.struct_values.iter().any(|(id, _, _)| *id == value_id)
                || self.bool_values.contains(&value_id)
                || (*parameter_type == Type::Integer(32)) != self.i32_values.contains(&value_id)
        };
        if invalid_value(lhs_id) || invalid_value(rhs_id) {
            return u64::MAX;
        }
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        if callee.parameters.len() != 2
            || callee.parameter_types.iter().any(|ty| ty != parameter_type)
            || callee.result_type != Type::Unit
        {
            return u64::MAX;
        }
        let callee_name = callee.reference();
        let callee_module = callee.module;
        let Some(Some(host)) = self.functions.get_mut(host_index) else {
            return u64::MAX;
        };
        if host.module != callee_module || host.builder.is_terminated() {
            return u64::MAX;
        }
        host.builder.call_void(callee_name, vec![lhs, rhs]);
        0
    }

    fn function_call_void_typed(&mut self, arguments: &[u64], parameter_type: &Type) -> u64 {
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
        let Some(value) = self.value_name(host_id, Some(value_id)) else {
            return u64::MAX;
        };
        if self
            .arrays
            .iter()
            .any(|(id, array)| *id == value_id && array.function == host_id)
            || self.enum_values.iter().any(|(id, _)| *id == value_id)
            || self.struct_values.iter().any(|(id, _, _)| *id == value_id)
            || self.bool_values.contains(&value_id)
            || (*parameter_type == Type::Integer(32)) != self.i32_values.contains(&value_id)
        {
            return u64::MAX;
        }
        let Some(Some(callee)) = self.functions.get(callee_index) else {
            return u64::MAX;
        };
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(parameter_type)
            || callee.result_type != Type::Unit
        {
            return u64::MAX;
        }
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
