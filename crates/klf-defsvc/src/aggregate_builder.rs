use super::{BuilderValue, DefinitionServiceState};
use klf_core::Name;
use std::sync::{Arc, Mutex};

pub(super) struct BuilderArray {
    pub(super) function: u64,
    pub(super) name: Name,
    pub(super) element: klf_kir::Type,
    pub(super) length: usize,
    pub(super) initialized: Vec<bool>,
}

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_value_pair_i64",
        state,
        DefinitionServiceState::value_pair_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_pair_i32",
        state,
        DefinitionServiceState::value_pair_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_pair_get_i64",
        state,
        DefinitionServiceState::value_pair_get_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_pair_get_i32",
        state,
        DefinitionServiceState::value_pair_get_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_pair_i64",
        state,
        DefinitionServiceState::function_param_pair_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_param_pair_i32",
        state,
        DefinitionServiceState::function_param_pair_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_i64",
        state,
        DefinitionServiceState::value_array_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_set_i64",
        state,
        DefinitionServiceState::value_array_set_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_get_i64",
        state,
        DefinitionServiceState::value_array_get_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_i32",
        state,
        DefinitionServiceState::value_array_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_set_i32",
        state,
        DefinitionServiceState::value_array_set_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_value_array_get_i32",
        state,
        DefinitionServiceState::value_array_get_i32,
    );
}

impl DefinitionServiceState {
    pub(crate) fn pair_type(element: klf_kir::Type) -> klf_kir::Type {
        klf_kir::Type::Address(Box::new(klf_kir::Type::Array(2, Box::new(element))))
    }

    pub(crate) fn pair_element(&self, function: u64, value: Option<u64>) -> Option<klf_kir::Type> {
        let value = value?;
        self.arrays
            .iter()
            .find(|(id, array)| *id == value && array.function == function)
            .and_then(|(_, array)| (array.length == 2).then(|| array.element.clone()))
    }

    fn function_param_pair_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_param_pair(arguments, klf_kir::Type::Integer(64))
    }

    fn function_param_pair_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_param_pair(arguments, klf_kir::Type::Integer(32))
    }

    fn function_param_pair(&mut self, arguments: &[u64], element: klf_kir::Type) -> u64 {
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
        let pair_type = Self::pair_type(element.clone());
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(name) = self
            .functions
            .get(function_index)
            .and_then(Option::as_ref)
            .filter(|function| {
                function.parameter_types.get(index) == Some(&pair_type)
                    && !function.builder.is_terminated()
            })
            .and_then(|function| function.parameters.get(index).cloned())
        else {
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
            BuilderArray {
                function: function_id,
                name,
                element,
                length: 2,
                initialized: vec![true, true],
            },
        ));
        id
    }

    fn value_pair_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_pair(arguments, klf_kir::Type::Integer(64))
    }

    fn value_pair_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_pair(arguments, klf_kir::Type::Integer(32))
    }

    fn value_pair(&mut self, arguments: &[u64], element: klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(left) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(right) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(left_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(right_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        let left_i32 = self.i32_values.contains(&left_id);
        let right_i32 = self.i32_values.contains(&right_id);
        if self.bool_values.contains(&left_id)
            || self.bool_values.contains(&right_id)
            || (element == klf_kir::Type::Integer(32)) != left_i32
            || left_i32 != right_i32
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
        let aggregate = function
            .builder
            .stack_alloc(klf_kir::Type::Array(2, Box::new(element.clone())));
        let aggregate_name = aggregate.clone();
        let zero = function.builder.const_int(klf_kir::Type::Integer(64), 0);
        let one = function.builder.const_int(klf_kir::Type::Integer(64), 1);
        let left_address = function.builder.elem_addr(aggregate.clone(), zero);
        function
            .builder
            .store(klf_kir::StoreMode::Init, left, left_address);
        let right_address = function.builder.elem_addr(aggregate.clone(), one);
        function
            .builder
            .store(klf_kir::StoreMode::Init, right, right_address);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: aggregate,
        }));
        self.arrays.push((
            id,
            BuilderArray {
                function: function_id,
                name: aggregate_name,
                element,
                length: 2,
                initialized: vec![true, true],
            },
        ));
        id
    }

    fn value_pair_get_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_pair_get(arguments, &klf_kir::Type::Integer(64))
    }

    fn value_pair_get_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_pair_get(arguments, &klf_kir::Type::Integer(32))
    }

    fn value_pair_get(&mut self, arguments: &[u64], element: &klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(pair_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some((_, array)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == pair_id && array.function == function_id)
        else {
            return u64::MAX;
        };
        let Some(pair) = self.value_name(function_id, Some(pair_id)) else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        if array.element != *element || index >= array.length || !array.initialized[index] {
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
        let index = function.builder.const_int(
            klf_kir::Type::Integer(64),
            i128::try_from(index).unwrap_or(0),
        );
        let address = function.builder.elem_addr(pair, index);
        let name = function.builder.load(address);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *element == klf_kir::Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    fn value_array_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_new(arguments, klf_kir::Type::Integer(64))
    }

    fn value_array_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_new(arguments, klf_kir::Type::Integer(32))
    }

    fn value_array_new(&mut self, arguments: &[u64], element: klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
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
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let Ok(kir_length) = u64::try_from(length) else {
            return u64::MAX;
        };
        let aggregate = function
            .builder
            .stack_alloc(klf_kir::Type::Array(kir_length, Box::new(element.clone())));
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name: aggregate.clone(),
        }));
        self.arrays.push((
            id,
            BuilderArray {
                function: function_id,
                name: aggregate,
                element,
                length,
                initialized: vec![false; length],
            },
        ));
        id
    }

    fn value_array_set_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_set(arguments, &klf_kir::Type::Integer(64))
    }

    fn value_array_set_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_set(arguments, &klf_kir::Type::Integer(32))
    }

    fn value_array_set(&mut self, arguments: &[u64], element: &klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(array_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some((_, array)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == array_id && array.function == function_id)
        else {
            return u64::MAX;
        };
        if array.element != *element || index >= array.length || array.initialized[index] {
            return u64::MAX;
        }
        let array_name = array.name.clone();
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let index_name = function.builder.const_int(
            klf_kir::Type::Integer(64),
            i128::try_from(index).unwrap_or(0),
        );
        let address = function.builder.elem_addr(array_name, index_name);
        function
            .builder
            .store(klf_kir::StoreMode::Init, value, address);
        if let Some((_, array)) = self
            .arrays
            .iter_mut()
            .find(|(id, array)| *id == array_id && array.function == function_id)
        {
            array.initialized[index] = true;
        }
        0
    }

    fn value_array_get_i64(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_get(arguments, &klf_kir::Type::Integer(64))
    }

    fn value_array_get_i32(&mut self, arguments: &[u64]) -> u64 {
        self.value_array_get(arguments, &klf_kir::Type::Integer(32))
    }

    fn value_array_get(&mut self, arguments: &[u64], element: &klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(array_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(index) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        let Some((_, array)) = self
            .arrays
            .iter()
            .find(|(id, array)| *id == array_id && array.function == function_id)
        else {
            return u64::MAX;
        };
        if array.element != *element || index >= array.length || !array.initialized[index] {
            return u64::MAX;
        }
        let array_name = array.name.clone();
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let index_name = function.builder.const_int(
            klf_kir::Type::Integer(64),
            i128::try_from(index).unwrap_or(0),
        );
        let address = function.builder.elem_addr(array_name, index_name);
        let name = function.builder.load(address);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *element == klf_kir::Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }
}
