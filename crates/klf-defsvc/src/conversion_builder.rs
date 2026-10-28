use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{ConvertKind, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_value_i32_to_i64",
        state,
        move |state, arguments| {
            state.value_convert(arguments, ConvertKind::SignExtend, &Type::Integer(64))
        },
    );
    capabilities.register_state(
        "klfsvc_kir_value_i64_to_i32",
        state,
        move |state, arguments| {
            state.value_convert(arguments, ConvertKind::Truncate, &Type::Integer(32))
        },
    );
    let i64_to_f64 = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_i64_to_f64", move |arguments| {
        i64_to_f64.lock().map_or(u64::MAX, |mut state| {
            state.value_convert(arguments, ConvertKind::SignedToFloat, &Type::Float(64))
        })
    });
    let f64_to_i64 = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_f64_to_i64", move |arguments| {
        f64_to_i64.lock().map_or(u64::MAX, |mut state| {
            state.value_convert(arguments, ConvertKind::FloatToSigned, &Type::Integer(64))
        })
    });
}

impl DefinitionServiceState {
    fn value_convert(&mut self, arguments: &[u64], kind: ConvertKind, ty: &Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(value) = self.value_name(function_id, arguments.get(1).copied()) else {
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
        let name = function.builder.convert(kind, value, ty.clone());
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        // Typed like the low-level builder's values, so its binary and
        // compare operations accept the result.
        self.set_value_type(id, function_id, ty.clone());
        if *ty == Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }
}
