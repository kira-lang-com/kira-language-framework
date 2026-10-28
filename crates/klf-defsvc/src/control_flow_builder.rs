//! General control flow and declarations for the low-level KIR builder.
//!
//! Blocks, their parameters and the edge arguments of every terminator
//! accumulate against a function handle and are drained by the service that
//! commits them, the same shape the call and signature services use. Nothing
//! here knows a hosted language's control-flow constructs.

use super::low_level_builder::signed;
use super::{BuilderBlock, BuilderFunction, DefinitionServiceState};
use klf_core::Name;
use klf_kir::{BlockTarget, Type};
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
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_parameter",
        ll_block_parameter
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_define",
        ll_block_define
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_entry",
        ll_block_entry
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_value",
        ll_block_value
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_switch",
        ll_block_switch
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_else_argument",
        ll_block_else_argument
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_branch",
        ll_block_branch
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_conditional_branch",
        ll_block_conditional_branch
    );
    service!(state, capabilities, "klfsvc_kir_block_trap", ll_block_trap);
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_trap_text_byte",
        ll_block_trap_text_byte
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_block_unreachable",
        ll_block_unreachable
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_switch_case",
        ll_switch_case
    );
    service!(state, capabilities, "klfsvc_kir_switch_int", ll_switch_int);
    service!(
        state,
        capabilities,
        "klfsvc_kir_signature_declare",
        ll_signature_declare
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_initializer",
        ll_function_initializer
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_body",
        ll_function_body
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_function_at",
        ll_function_at
    );
}

impl DefinitionServiceState {
    /// A function handle regardless of whether its current block is already
    /// terminated. Creating a block or moving the cursor stays legal after a
    /// terminator; emitting into the block does not.
    fn kir_open_function_mut(&mut self, function: u64) -> Option<&mut BuilderFunction> {
        let index = usize::try_from(function).ok()?;
        self.functions.get_mut(index)?.as_mut()
    }

    fn kir_block(&self, function: u64, block: Option<u64>) -> Option<&BuilderBlock> {
        let index = usize::try_from(block?).ok()?;
        let block = self.blocks.get(index)?.as_ref()?;
        (block.function == function).then_some(block)
    }

    fn ll_block_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        if self.kir_open_function_mut(function_id).is_none() {
            return u64::MAX;
        }
        self.kir_block_parameters.push((function_id, ty));
        0
    }

    fn ll_take_block_parameters(&mut self, function_id: u64) -> Vec<Type> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for (owner, ty) in self.kir_block_parameters.drain(..) {
            if owner == function_id {
                taken.push(ty);
            } else {
                kept.push((owner, ty));
            }
        }
        self.kir_block_parameters = kept;
        taken
    }

    fn ll_block_define(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        if self.kir_open_function_mut(function_id).is_none() {
            return u64::MAX;
        }
        let parameter_types = self.ll_take_block_parameters(function_id);
        let id = u64::try_from(self.blocks.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        let Some(function) = self.kir_open_function_mut(function_id) else {
            return u64::MAX;
        };
        let parameters: Vec<Name> = (0..parameter_types.len())
            .map(|_| function.builder.fresh_value())
            .collect();
        let declared: Vec<(Name, Type)> = parameters
            .iter()
            .cloned()
            .zip(parameter_types.iter().cloned())
            .collect();
        let name = function.builder.new_block(&declared);
        self.blocks.push(Some(BuilderBlock {
            function: function_id,
            name,
            parameters,
            parameter_types,
        }));
        id
    }

    /// A handle to the block a function starts in; its parameters are the
    /// function parameters. A definition that wants every stack slot to
    /// dominate the whole body emits the body into a block of its own and
    /// switches back here whenever it needs a slot.
    fn ll_block_entry(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(function) = self.kir_open_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.entry_block();
        let parameters = function.parameters.clone();
        let parameter_types = function.parameter_types.clone();
        let id = u64::try_from(self.blocks.len()).unwrap_or(u64::MAX);
        if id != u64::MAX {
            self.blocks.push(Some(BuilderBlock {
                function: function_id,
                name,
                parameters,
                parameter_types,
            }));
        }
        id
    }

    fn ll_block_value(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(position) = signed(arguments.get(2)).and_then(|p| usize::try_from(p).ok()) else {
            return u64::MAX;
        };
        let Some(block) = self.kir_block(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(name) = block.parameters.get(position).cloned() else {
            return u64::MAX;
        };
        let Some(ty) = block.parameter_types.get(position).cloned() else {
            return u64::MAX;
        };
        self.kir_new_value(function_id, name, ty)
    }

    fn ll_block_switch(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(name) = self
            .kir_block(function_id, arguments.get(1).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let Some(function) = self.kir_open_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.switch_to(&name);
        0
    }

    fn ll_block_else_argument(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        self.kir_else_arguments.push((function_id, value));
        0
    }

    fn ll_take_else_arguments(&mut self, function_id: u64) -> Vec<Name> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for (owner, value) in self.kir_else_arguments.drain(..) {
            if owner == function_id {
                taken.push(value);
            } else {
                kept.push((owner, value));
            }
        }
        self.kir_else_arguments = kept;
        taken
    }

    fn ll_block_branch(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(target) = self
            .kir_block(function_id, arguments.get(1).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let edge = self.ll_take_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.branch(target, edge);
        0
    }

    fn ll_block_conditional_branch(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(condition) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(then_block) = self
            .kir_block(function_id, arguments.get(2).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let Some(else_block) = self
            .kir_block(function_id, arguments.get(3).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let then_arguments = self.ll_take_arguments(function_id);
        let else_arguments = self.ll_take_else_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.conditional_branch(
            condition,
            then_block,
            then_arguments,
            else_block,
            else_arguments,
        );
        0
    }

    fn ll_block_trap(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(message) = self.trap_report(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.trap(message);
        0
    }

    fn ll_block_unreachable(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let message = usize::try_from(function_id)
            .ok()
            .and_then(|index| self.functions.get(index))
            .and_then(Option::as_ref)
            .map(|function| self.unreachable_message(&function.builder));
        let (Some(message), Some(function)) = (message, self.kir_function_mut(function_id)) else {
            return u64::MAX;
        };
        function.builder.trap(message);
        0
    }

    fn ll_switch_case(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = signed(arguments.get(1)) else {
            return u64::MAX;
        };
        let Some(target) = self
            .kir_block(function_id, arguments.get(2).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let edge = self.ll_take_arguments(function_id);
        self.kir_switch_cases
            .push((function_id, i128::from(value), target, edge));
        0
    }

    fn ll_switch_int(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(default_block) = self
            .kir_block(function_id, arguments.get(2).copied())
            .map(|block| block.name.clone())
        else {
            return u64::MAX;
        };
        let default_arguments = self.ll_take_arguments(function_id);
        let mut cases = Vec::new();
        let mut kept = Vec::new();
        for (owner, case, block, edge) in self.kir_switch_cases.drain(..) {
            if owner == function_id {
                cases.push((
                    case,
                    BlockTarget {
                        block,
                        arguments: edge,
                    },
                ));
            } else {
                kept.push((owner, case, block, edge));
            }
        }
        self.kir_switch_cases = kept;
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.switch_int(
            value,
            cases,
            BlockTarget {
                block: default_block,
                arguments: default_arguments,
            },
        );
        0
    }

    fn ll_signature_declare(&mut self, arguments: &[u64]) -> u64 {
        self.ll_signature_commit(arguments, true)
    }

    fn ll_function_initializer(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(function) = self.kir_open_function_mut(function_id) else {
            return u64::MAX;
        };
        function
            .builder
            .add_attribute(klf_kir::FunctionAttribute::Initializer);
        0
    }

    fn ll_function_body(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(function) = self.kir_open_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.set_foreign(false);
        0
    }
}
