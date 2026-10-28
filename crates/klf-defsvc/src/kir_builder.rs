use super::{BuilderBlock, BuilderValue, DefinitionServiceState};
use klf_core::Name;
use klf_kir::{BlockTarget, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    register_calls(state, capabilities);
    register_control(state, capabilities);
}

/// The call and pair services.
fn register_calls(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_call_i64",
        state,
        DefinitionServiceState::function_call_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_i32",
        state,
        DefinitionServiceState::function_call_i32,
    );
    let function_call_f64 = Arc::clone(state);
    capabilities.register("klfsvc_kir_function_call_f64", move |arguments| {
        function_call_f64
            .lock()
            .map_or(u64::MAX, |mut state| state.function_call_f64(arguments))
    });
    capabilities.register_state(
        "klfsvc_kir_function_call_pair_i64",
        state,
        DefinitionServiceState::function_call_pair_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call_pair_i32",
        state,
        DefinitionServiceState::function_call_pair_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_pair_i64",
        state,
        DefinitionServiceState::function_new_pair_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_new_pair_i32",
        state,
        DefinitionServiceState::function_new_pair_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_call2_i64",
        state,
        move |state, arguments| state.function_call_typed(arguments, &Type::Integer(64), 2),
    );
    capabilities.register_state(
        "klfsvc_kir_function_call2_i32",
        state,
        move |state, arguments| state.function_call_typed(arguments, &Type::Integer(32), 2),
    );
}

/// Switches, blocks and branches.
fn register_control(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_function_switch_i64",
        state,
        DefinitionServiceState::function_switch_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_switch_i32",
        state,
        DefinitionServiceState::function_switch_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_switch_enum",
        state,
        DefinitionServiceState::function_switch_enum,
    );
    capabilities.register_state(
        "klfsvc_kir_function_block_new",
        state,
        DefinitionServiceState::function_block_new,
    );
    capabilities.register_state(
        "klfsvc_kir_function_block_new_i64",
        state,
        DefinitionServiceState::function_block_new_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_block_new_i32",
        state,
        DefinitionServiceState::function_block_new_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_block_param_i64",
        state,
        DefinitionServiceState::function_block_param_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_block_param_i32",
        state,
        DefinitionServiceState::function_block_param_i32,
    );
    capabilities.register_state(
        "klfsvc_kir_function_switch_block",
        state,
        DefinitionServiceState::function_switch_block,
    );
    capabilities.register_state(
        "klfsvc_kir_function_branch",
        state,
        DefinitionServiceState::function_branch,
    );
    capabilities.register_state(
        "klfsvc_kir_function_branch_i64",
        state,
        DefinitionServiceState::function_branch_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_branch_i32",
        state,
        DefinitionServiceState::function_branch_i32,
    );
}

impl DefinitionServiceState {
    fn function_new_pair_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_pair(arguments, Type::Integer(64))
    }

    fn function_new_pair_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_new_pair(arguments, Type::Integer(32))
    }

    fn function_new_pair(&mut self, arguments: &[u64], result_type: Type) -> u64 {
        let Some(count) = arguments
            .get(3)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
            .and_then(|value| usize::try_from(value).ok())
        else {
            return u64::MAX;
        };
        if count != 1 {
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
        let parameter_type = DefinitionServiceState::pair_type(result_type.clone());
        let parameters = vec![(Name::new("arg0"), parameter_type.clone())];
        let id = u64::try_from(self.functions.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.functions.push(Some(super::BuilderFunction {
            module,
            name: name.clone(),
            builder: klf_services::FunctionBuilder::new(
                name,
                &parameters,
                result_type.clone(),
                span,
            ),
            result_type,
            parameters: vec![Name::new("arg0")],
            parameter_types: vec![parameter_type],
            referenced: std::cell::Cell::new(false),
        }));
        id
    }

    fn function_block_new(&mut self, arguments: &[u64]) -> u64 {
        self.function_block_new_with_parameters(arguments, 0, &Type::Integer(64))
    }

    fn function_block_new_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_block_new_with_parameters(arguments, 1, &Type::Integer(64))
    }

    fn function_block_new_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_block_new_with_parameters(arguments, 1, &Type::Integer(32))
    }

    fn function_block_new_with_parameters(
        &mut self,
        arguments: &[u64],
        count: usize,
        ty: &Type,
    ) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        let parameters: Vec<_> = (0..count)
            .map(|index| (Name::new(format!("block_arg{index}")), ty.clone()))
            .collect();
        let parameter_names = parameters.iter().map(|(name, _)| name.clone()).collect();
        let parameter_types = parameters.iter().map(|(_, ty)| ty.clone()).collect();
        let name = function.builder.new_block(&parameters);
        let id = u64::try_from(self.blocks.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.blocks.push(Some(BuilderBlock {
            function: function_id,
            name,
            parameters: parameter_names,
            parameter_types,
        }));
        id
    }

    fn function_block_param_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_block_param(arguments, &Type::Integer(64))
    }

    fn function_block_param_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_block_param(arguments, &Type::Integer(32))
    }

    fn function_block_param(&mut self, arguments: &[u64], ty: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(block_id) = arguments.get(1).copied() else {
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
        let Ok(block_index) = usize::try_from(block_id) else {
            return u64::MAX;
        };
        let Some(Some(block)) = self.blocks.get(block_index) else {
            return u64::MAX;
        };
        if block.function != function_id || block.parameter_types.get(index) != Some(ty) {
            return u64::MAX;
        }
        let Some(name) = block.parameters.get(index).cloned() else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
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

    fn function_switch_block(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(block) = self.block_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        function.builder.switch_to(&block);
        0
    }

    fn function_branch(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(block) = self.block_name(function_id, arguments.get(1).copied()) else {
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
        function.builder.branch(block, Vec::new());
        0
    }

    fn function_branch_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_branch_value(arguments, None)
    }

    fn function_branch_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_branch_value(arguments, Some(Type::Integer(32)))
    }

    fn function_branch_value(&mut self, arguments: &[u64], parameter_type: Option<Type>) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(block) = self.block_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        if let Some(parameter_type) = parameter_type {
            if parameter_type == Type::Integer(32) && !self.i32_values.contains(&value_id) {
                return u64::MAX;
            }
            let Some(block_id) = arguments.get(1).copied() else {
                return u64::MAX;
            };
            let Ok(block_index) = usize::try_from(block_id) else {
                return u64::MAX;
            };
            let Some(Some(block)) = self.blocks.get(block_index) else {
                return u64::MAX;
            };
            if block.function != function_id
                || block.parameter_types.first() != Some(&parameter_type)
            {
                return u64::MAX;
            }
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
        function.builder.branch(block, vec![value]);
        0
    }

    pub(super) fn block_name(&self, function: u64, block: Option<u64>) -> Option<Name> {
        let block = usize::try_from(block?).ok()?;
        let block = self.blocks.get(block)?.as_ref()?;
        (block.function == function).then(|| block.name.clone())
    }

    pub(super) fn block_parameter(
        &self,
        function: u64,
        block: Option<u64>,
        ty: &Type,
    ) -> Option<Name> {
        let block = usize::try_from(block?).ok()?;
        let block = self.blocks.get(block)?.as_ref()?;
        (block.function == function && block.parameter_types.first() == Some(ty))
            .then(|| block.parameters.first().cloned())
            .flatten()
    }

    pub(super) fn function_call_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_typed(arguments, &Type::Integer(64), 1)
    }

    fn function_call_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_typed(arguments, &Type::Integer(32), 1)
    }

    fn function_call_f64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_typed(arguments, &Type::Float(64), 1)
    }

    fn function_call_pair_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_pair(arguments, &Type::Integer(64))
    }

    fn function_call_pair_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_call_pair(arguments, &Type::Integer(32))
    }

    fn function_call_pair(&mut self, arguments: &[u64], result_type: &Type) -> u64 {
        let Some(host_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(pair_id) = arguments.get(2).copied() else {
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
        let pair_type = DefinitionServiceState::pair_type(result_type.clone());
        if callee.parameters.len() != 1
            || callee.parameter_types.first() != Some(&pair_type)
            || callee.result_type != *result_type
        {
            return u64::MAX;
        }
        let Some(pair) = self.value_name(host_id, Some(pair_id)) else {
            return u64::MAX;
        };
        if self.pair_element(host_id, Some(pair_id)) != Some(result_type.clone()) {
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
        let name = host.builder.call(callee_name, vec![pair]);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: host_id,
            name,
        }));
        if *result_type == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    fn function_call_typed(&mut self, arguments: &[u64], ty: &Type, arity: usize) -> u64 {
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
        if callee.parameters.len() != arity
            || callee.result_type != *ty
            || callee
                .parameter_types
                .iter()
                .any(|parameter| parameter != ty)
        {
            return u64::MAX;
        }
        let Some(values) = (0..arity)
            .map(|index| self.value_name(host_id, arguments.get(index + 2).copied()))
            .collect::<Option<Vec<_>>>()
        else {
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
        let name = host.builder.call(callee_name, values);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: host_id,
            name,
        }));
        if *ty == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    fn function_switch_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_switch_typed(arguments, &Type::Integer(64))
    }

    fn function_switch_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_switch_typed(arguments, &Type::Integer(32))
    }

    fn function_switch_enum(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, Some(value_id)) else {
            return u64::MAX;
        };
        if !self.enum_values.iter().any(|(id, _)| *id == value_id) {
            return u64::MAX;
        }
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
            .find(|(id, _)| *id == value_id)
            .map(|(_, type_id)| *type_id)
        else {
            return u64::MAX;
        };
        let Ok(type_index) = usize::try_from(type_id) else {
            return u64::MAX;
        };
        if self
            .types
            .get(type_index)
            .and_then(|(_, declaration)| declaration.variants.get(variant as usize))
            .is_none()
        {
            return u64::MAX;
        }
        let Some(case_block) = self.block_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(default_block) = self.block_name(function_id, arguments.get(4).copied()) else {
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
        function.builder.switch_enum(
            value,
            vec![(
                variant,
                BlockTarget {
                    block: case_block,
                    arguments: Vec::new(),
                },
            )],
            Some(BlockTarget {
                block: default_block,
                arguments: Vec::new(),
            }),
        );
        0
    }

    fn function_switch_typed(&mut self, arguments: &[u64], ty: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(value_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        if (*ty == Type::Integer(32)) != self.i32_values.contains(&value_id)
            || self.bool_values.contains(&value_id)
        {
            return u64::MAX;
        }
        let Some(case_value) = arguments
            .get(2)
            .copied()
            .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
        else {
            return u64::MAX;
        };
        if *ty == Type::Integer(32) && i32::try_from(case_value).is_err() {
            return u64::MAX;
        }
        let Some(case_block) = self.block_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(default_block) = self.block_name(function_id, arguments.get(4).copied()) else {
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
        function.builder.switch_int(
            value,
            vec![(
                i128::from(case_value),
                BlockTarget {
                    block: case_block,
                    arguments: Vec::new(),
                },
            )],
            BlockTarget {
                block: default_block,
                arguments: Vec::new(),
            },
        );
        0
    }
}
