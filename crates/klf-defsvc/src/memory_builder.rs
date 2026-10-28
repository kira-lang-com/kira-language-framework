use super::{BuilderAddress, BuilderValue, DefinitionServiceState};
use klf_kir::{StoreMode, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_value_stack_alloc_i64",
        state,
        DefinitionServiceState::value_stack_alloc_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_store_i64",
        state,
        DefinitionServiceState::value_store_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_assign_i64",
        state,
        DefinitionServiceState::value_assign_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_stack_alloc_i32",
        state,
        DefinitionServiceState::value_stack_alloc_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_store_i32",
        state,
        DefinitionServiceState::value_store_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_assign_i32",
        state,
        DefinitionServiceState::value_assign_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_load_i32",
        state,
        DefinitionServiceState::value_load_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_load_i64",
        state,
        DefinitionServiceState::value_load_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_stack_dealloc_i64",
        state,
        DefinitionServiceState::value_stack_dealloc_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_stack_dealloc_i32",
        state,
        DefinitionServiceState::value_stack_dealloc_i32,
    );
}

impl DefinitionServiceState {
    fn value_stack_alloc_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_stack_alloc(arguments, Type::Integer(64))
    }

    fn value_stack_alloc_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_stack_alloc(arguments, Type::Integer(32))
    }

    fn value_stack_dealloc_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_stack_dealloc(arguments, &Type::Integer(64))
    }

    fn value_stack_dealloc_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_stack_dealloc(arguments, &Type::Integer(32))
    }

    fn value_stack_dealloc(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(address_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Ok(address_index) = usize::try_from(address_id) else {
            return u64::MAX;
        };
        let Some(Some(address)) = self.addresses.get(address_index) else {
            return u64::MAX;
        };
        if address.function != function_id || address.element != *element {
            return u64::MAX;
        }
        let address_name = address.name.clone();
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        function.builder.stack_dealloc(address_name);
        self.addresses[address_index] = None;
        0
    }

    fn value_stack_alloc(&mut self, arguments: &[u64], element: Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
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
        let name = function.builder.stack_alloc(element.clone());
        let id = u64::try_from(self.addresses.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.addresses.push(Some(BuilderAddress {
            function: function_id,
            name,
            element,
            initialized: false,
        }));
        id
    }

    fn value_store_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_store(arguments, &Type::Integer(64), StoreMode::Init)
    }

    fn value_assign_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_store(arguments, &Type::Integer(64), StoreMode::Assign)
    }

    fn value_store_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_store(arguments, &Type::Integer(32), StoreMode::Init)
    }

    fn value_assign_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_store(arguments, &Type::Integer(32), StoreMode::Assign)
    }

    fn value_store(&mut self, arguments: &[u64], element: &Type, mode: StoreMode) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(address_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let is_i32 = self.i32_values.contains(&value_id);
        let is_aggregate = self
            .arrays
            .iter()
            .any(|(id, array)| *id == value_id && array.function == function_id);
        if is_aggregate
            || self.bool_values.contains(&value_id)
            || (*element == Type::Integer(32)) != is_i32
        {
            return u64::MAX;
        }
        let Ok(address_index) = usize::try_from(address_id) else {
            return u64::MAX;
        };
        let Some(Some(address)) = self.addresses.get(address_index) else {
            return u64::MAX;
        };
        if address.function != function_id
            || address.element != *element
            || matches!(mode, StoreMode::Init) && address.initialized
            || matches!(mode, StoreMode::Assign) && !address.initialized
        {
            return u64::MAX;
        }
        let address_name = address.name.clone();
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        function.builder.store(mode, value, address_name);
        if let Some(Some(address)) = self.addresses.get_mut(address_index) {
            address.initialized = true;
        }
        0
    }

    fn value_load_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_load(arguments, &Type::Integer(64))
    }

    fn value_load_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_load(arguments, &Type::Integer(32))
    }

    fn value_load(&mut self, arguments: &[u64], element: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(address_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Ok(address_index) = usize::try_from(address_id) else {
            return u64::MAX;
        };
        let Some(Some(address)) = self.addresses.get(address_index) else {
            return u64::MAX;
        };
        if address.function != function_id || address.element != *element || !address.initialized {
            return u64::MAX;
        }
        let address_name = address.name.clone();
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let name = function.builder.load(address_name);
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
}
