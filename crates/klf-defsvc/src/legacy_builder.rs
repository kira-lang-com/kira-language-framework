//! Registration for the first-generation monomorphised builder services.
//!
//! These predate the general low-level builder and stay wired for the
//! definitions that still use them. Their implementations live on
//! `DefinitionServiceState` next to the session plumbing they grew out of.

use super::{BuilderFunction, BuilderValue, DefinitionServiceState};
use klf_core::Name;
use klf_kir::{BinaryOperator, Overflow, Type};
use klf_services::FunctionBuilder;
use std::sync::{Arc, Mutex};

macro_rules! service {
    ($state:expr, $capabilities:expr, $name:literal, $method:ident) => {{
        let shared = Arc::clone($state);
        $capabilities.register($name, move |arguments| {
            shared
                .lock()
                .map(|mut state| state.$method(arguments))
                .unwrap_or(u64::MAX)
        });
    }};
}

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    service!(state, capabilities, "klfsvc_kir_function_new", function_new);
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_return_int",
        function_return_int
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_const_int",
        value_const_int
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_const_i32",
        value_const_i32
    );
    service!(state, capabilities, "klfsvc_kir_value_add", value_add);
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_return",
        function_return
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_new_i64",
        function_new_i64
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_new_i32",
        function_new_i32
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_param_i64",
        function_param_i64
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_param_i32",
        function_param_i32
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_new_f64",
        function_new_f64
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_param_f64",
        function_param_f64
    );
}

impl DefinitionServiceState {
    pub(super) fn function_new(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_with_type(arguments, 0, Type::Integer(64))
    }

    pub(super) fn function_new_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(count) = arguments
            .get(3)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        self.function_new_with_type(arguments, count, Type::Integer(64))
    }

    pub(super) fn function_new_i32(&mut self, arguments: &[u64]) -> u64 {
        let Some(count) = arguments
            .get(3)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        self.function_new_with_type(arguments, count, Type::Integer(32))
    }

    pub(super) fn function_new_f64(&mut self, arguments: &[u64]) -> u64 {
        let Some(count) = arguments
            .get(3)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        self.function_new_with_type(arguments, count, Type::Float(64))
    }

    pub(super) fn function_new_with_type(
        &mut self,
        arguments: &[u64],
        count: usize,
        result_type: Type,
    ) -> u64 {
        if count > 32 {
            return u64::MAX;
        }
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
        let parameters: Vec<_> = (0..count)
            .map(|index| (Name::new(format!("arg{index}")), result_type.clone()))
            .collect();
        let parameter_names = parameters.iter().map(|(name, _)| name.clone()).collect();
        let parameter_types = parameters.iter().map(|(_, ty)| ty.clone()).collect();
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(BuilderFunction {
            module,
            name: name.clone(),
            builder: FunctionBuilder::new(name, &parameters, result_type.clone(), span),
            result_type,
            parameters: parameter_names,
            parameter_types,
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    pub(super) fn function_return_int(&mut self, arguments: &[u64]) -> u64 {
        let Some(function) = arguments.first().copied() else {
            return u64::MAX;
        };
        let value = arguments.get(1).copied().unwrap_or(0);
        let value_handle = self.value_const_int(&[function, value]);
        if value_handle == u64::MAX {
            return value_handle;
        }
        self.function_return(&[function, value_handle])
    }

    pub(super) fn value_const_int(&mut self, arguments: &[u64]) -> u64 {
        self.value_const_typed(arguments, &Type::Integer(64))
    }

    pub(super) fn value_const_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_const_typed(arguments, &Type::Integer(32))
    }

    pub(super) fn value_const_typed(&mut self, arguments: &[u64], ty: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let value = arguments
            .get(1)
            .copied()
            .map_or(0, |value| i64::from_ne_bytes(value.to_ne_bytes()));
        if matches!(ty, Type::Integer(32)) && i32::try_from(value).is_err() {
            return u64::MAX;
        }
        let name = function.builder.const_int(ty.clone(), i128::from(value));
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *ty == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    pub(super) fn value_add(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(lhs) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Ok(index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let name = function
            .builder
            .binary(BinaryOperator::Add, Overflow::Wrap, true, lhs, rhs);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        id
    }

    pub(super) fn function_return(&mut self, arguments: &[u64]) -> u64 {
        let Some(function) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Ok(index) = usize::try_from(function) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        function.builder.ret(Some(value));
        0
    }

    pub(super) fn function_param_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_param(arguments, &Type::Integer(64))
    }

    pub(super) fn function_param_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_param(arguments, &Type::Integer(32))
    }

    pub(super) fn function_param_f64(&mut self, arguments: &[u64]) -> u64 {
        self.function_param(arguments, &Type::Float(64))
    }

    pub(super) fn function_param(&mut self, arguments: &[u64], ty: &Type) -> u64 {
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
        let Some(Some(function)) = self.functions.get(function_index) else {
            return u64::MAX;
        };
        if function.parameter_types.get(index) != Some(ty) {
            return u64::MAX;
        }
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let Some(name) = function.parameters.get(index).cloned() else {
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
        if *ty == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }
}
