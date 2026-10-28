use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{FloatBinaryOperator, FloatPredicate, FloatUnaryOperator, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    let constant = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_const_f64", move |arguments| {
        constant
            .lock()
            .map_or(u64::MAX, |mut state| state.value_const_f64(arguments))
    });
    let add = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_add_f64", move |arguments| {
        add.lock().map_or(u64::MAX, |mut state| {
            state.value_float_binary(arguments, FloatBinaryOperator::Add)
        })
    });
    let sub = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_sub_f64", move |arguments| {
        sub.lock().map_or(u64::MAX, |mut state| {
            state.value_float_binary(arguments, FloatBinaryOperator::Sub)
        })
    });
    let mul = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_mul_f64", move |arguments| {
        mul.lock().map_or(u64::MAX, |mut state| {
            state.value_float_binary(arguments, FloatBinaryOperator::Mul)
        })
    });
    let div = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_div_f64", move |arguments| {
        div.lock().map_or(u64::MAX, |mut state| {
            state.value_float_binary(arguments, FloatBinaryOperator::Div)
        })
    });
    for (symbol, predicate) in [
        ("klfsvc_kir_value_eq_f64", FloatPredicate::Eq),
        ("klfsvc_kir_value_ne_f64", FloatPredicate::Ne),
        ("klfsvc_kir_value_lt_f64", FloatPredicate::Lt),
        ("klfsvc_kir_value_le_f64", FloatPredicate::Le),
        ("klfsvc_kir_value_gt_f64", FloatPredicate::Gt),
        ("klfsvc_kir_value_ge_f64", FloatPredicate::Ge),
    ] {
        let compare = Arc::clone(state);
        capabilities.register(symbol, move |arguments| {
            compare.lock().map_or(u64::MAX, |mut state| {
                state.value_float_compare(arguments, predicate)
            })
        });
    }
    for (symbol, operator) in [
        ("klfsvc_kir_value_neg_f64", FloatUnaryOperator::Neg),
        ("klfsvc_kir_value_abs_f64", FloatUnaryOperator::Abs),
        ("klfsvc_kir_value_sqrt_f64", FloatUnaryOperator::Sqrt),
        ("klfsvc_kir_value_floor_f64", FloatUnaryOperator::Floor),
        ("klfsvc_kir_value_ceil_f64", FloatUnaryOperator::Ceil),
        ("klfsvc_kir_value_trunc_f64", FloatUnaryOperator::Trunc),
        ("klfsvc_kir_value_round_f64", FloatUnaryOperator::Round),
    ] {
        let unary = Arc::clone(state);
        capabilities.register(symbol, move |arguments| {
            unary.lock().map_or(u64::MAX, |mut state| {
                state.value_float_unary(arguments, operator)
            })
        });
    }
}

impl DefinitionServiceState {
    fn value_const_f64(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(bits) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let literal = format!("{:?}", f64::from_bits(bits));
        let Ok(function_index) = usize::try_from(function_id) else {
            return u64::MAX;
        };
        let Some(Some(function)) = self.functions.get_mut(function_index) else {
            return u64::MAX;
        };
        if function.builder.is_terminated() {
            return u64::MAX;
        }
        let name = function.builder.const_float(Type::Float(64), literal);
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

    fn value_float_unary(&mut self, arguments: &[u64], operator: FloatUnaryOperator) -> u64 {
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
        let name = function.builder.float_unary(operator, value);
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

    fn value_float_compare(&mut self, arguments: &[u64], predicate: FloatPredicate) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(lhs) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, arguments.get(2).copied()) else {
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
        let name = function.builder.float_compare(predicate, lhs, rhs);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        self.bool_values.insert(id);
        id
    }

    fn value_float_binary(&mut self, arguments: &[u64], operator: FloatBinaryOperator) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(lhs) = self.value_name(function_id, arguments.get(1).copied()) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, arguments.get(2).copied()) else {
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
        let name = function.builder.float_binary(operator, lhs, rhs);
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
}
