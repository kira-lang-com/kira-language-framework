//! Calls for the general low-level KIR builder: direct calls, function
//! references and indirect calls. Arguments and indirect parameter types
//! accumulate per function handle and are drained by the call that commits
//! them.

use super::DefinitionServiceState;
use klf_core::Name;
use klf_kir::Type;
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
        "klfsvc_kir_value_argument",
        ll_value_argument
    );
    service!(state, capabilities, "klfsvc_kir_value_call", ll_value_call);
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_call_void",
        ll_value_call_void
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_function_reference",
        ll_value_function_reference
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_indirect_parameter",
        ll_value_indirect_parameter
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_call_indirect",
        ll_value_call_indirect
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_call_indirect_void",
        ll_value_call_indirect_void
    );
}

impl DefinitionServiceState {
    fn ll_value_argument(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        self.kir_pending_arguments.push((function_id, value));
        0
    }

    pub(super) fn ll_take_arguments(&mut self, function_id: u64) -> Vec<Name> {
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for (owner, value) in self.kir_pending_arguments.drain(..) {
            if owner == function_id {
                taken.push(value);
            } else {
                kept.push((owner, value));
            }
        }
        self.kir_pending_arguments = kept;
        taken
    }

    pub(super) fn ll_callee_name(&self, callee: Option<u64>) -> Option<Name> {
        let index = usize::try_from(callee?).ok()?;
        self.functions.get(index)?.as_ref().map(|f| f.name.clone())
    }

    /// The name of `callee` for a call or reference to it.
    fn ll_referenced_callee(&self, callee: Option<u64>) -> Option<Name> {
        let index = usize::try_from(callee?).ok()?;
        self.functions
            .get(index)?
            .as_ref()
            .map(super::BuilderFunction::reference)
    }

    /// 1 when a call or reference names `function`, else 0.
    pub(super) fn function_referenced(&mut self, arguments: &[u64]) -> u64 {
        let function = arguments
            .first()
            .and_then(|function| usize::try_from(*function).ok());
        match function.and_then(|function| self.functions.get(function)) {
            Some(Some(function)) => u64::from(function.referenced.get()),
            Some(None) | None => u64::MAX,
        }
    }

    /// Drops `function`, declared but never named, from its module.
    pub(super) fn function_discard(&mut self, arguments: &[u64]) -> u64 {
        let function = arguments
            .first()
            .and_then(|function| usize::try_from(*function).ok());
        match function.and_then(|function| self.functions.get_mut(function)) {
            Some(slot @ Some(_))
                if slot
                    .as_ref()
                    .is_some_and(|function| !function.referenced.get()) =>
            {
                *slot = None;
                0
            }
            Some(_) | None => u64::MAX,
        }
    }

    fn ll_value_call(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee) = self.ll_referenced_callee(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(result_type) = self.kir_type(arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let call_arguments = self.ll_take_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.call(callee, call_arguments);
        self.kir_new_value(function_id, name, result_type)
    }

    fn ll_value_call_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee) = self.ll_referenced_callee(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let call_arguments = self.ll_take_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function.builder.call_void(callee, call_arguments);
        0
    }

    fn ll_value_function_reference(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(callee) = self.ll_referenced_callee(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name = function.builder.function_reference(callee);
        self.kir_new_value(function_id, name, Type::Pointer)
    }

    fn ll_value_indirect_parameter(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(arguments.get(1).copied()) else {
            return u64::MAX;
        };
        self.kir_pending_parameters.push((function_id, ty));
        0
    }

    fn ll_take_parameters(&mut self, function_id: u64) -> Vec<Type> {
        let mut parameters = Vec::new();
        let mut kept = Vec::new();
        for (owner, ty) in self.kir_pending_parameters.drain(..) {
            if owner == function_id {
                parameters.push(ty);
            } else {
                kept.push((owner, ty));
            }
        }
        self.kir_pending_parameters = kept;
        parameters
    }

    fn ll_value_call_indirect_void(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(target) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let parameters = self.ll_take_parameters(function_id);
        let call_arguments = self.ll_take_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        function
            .builder
            .call_indirect_void(target, parameters, call_arguments);
        0
    }

    fn ll_value_call_indirect(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(target) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(result_type) = self.kir_type(arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let parameters = self.ll_take_parameters(function_id);
        let call_arguments = self.ll_take_arguments(function_id);
        let Some(function) = self.kir_function_mut(function_id) else {
            return u64::MAX;
        };
        let name =
            function
                .builder
                .call_indirect(target, parameters, result_type.clone(), call_arguments);
        self.kir_new_value(function_id, name, result_type)
    }
}
