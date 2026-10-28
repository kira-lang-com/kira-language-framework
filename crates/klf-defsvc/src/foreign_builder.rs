use super::{BuilderValue, DefinitionServiceState};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    let call = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_call_service_i64", move |arguments| {
        call.lock().map_or(u64::MAX, |mut state| {
            state.value_call_service_i64(arguments)
        })
    });
    let call_one = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_call_service_i64_1", move |arguments| {
        call_one.lock().map_or(u64::MAX, |mut state| {
            state.value_call_service_i64_1(arguments)
        })
    });
    let call_void_one = Arc::clone(state);
    capabilities.register(
        "klfsvc_kir_function_call_service_void_i64_1",
        move |arguments| {
            call_void_one.lock().map_or(u64::MAX, |mut state| {
                state.function_call_service_void_i64_1(arguments)
            })
        },
    );
}

impl DefinitionServiceState {
    fn value_call_service_i64(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((symbol, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
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
        let name = function.builder.call_foreign(symbol, Vec::new());
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

    fn value_call_service_i64_1(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((symbol, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(argument_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(argument) = self.value_name(function_id, Some(argument_id)) else {
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
        let name = function.builder.call_foreign(symbol, vec![argument]);
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

    fn function_call_service_void_i64_1(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((symbol, _)) = self.source_name(arguments.get(1), arguments.get(2)) else {
            return u64::MAX;
        };
        let Some(argument_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(argument) = self.value_name(function_id, Some(argument_id)) else {
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
        function.builder.call_foreign_void(symbol, vec![argument]);
        0
    }
}
