use super::DefinitionServiceState;
use super::low_level_builder::signed;
use klf_kir::{FloatBinaryOperator, FloatPredicate, FloatUnaryOperator, Type};
use std::sync::{Arc, Mutex};

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, sink: &mut crate::ServiceSink) {
    for (name, handler) in [
        (
            "klfsvc_kir_value_float_literal",
            DefinitionServiceState::ll_float_literal
                as fn(&mut DefinitionServiceState, &[u64]) -> u64,
        ),
        (
            "klfsvc_kir_value_float_binary",
            DefinitionServiceState::ll_float_binary,
        ),
        (
            "klfsvc_kir_value_float_compare",
            DefinitionServiceState::ll_float_compare,
        ),
        (
            "klfsvc_kir_value_float_negate",
            DefinitionServiceState::ll_float_negate,
        ),
    ] {
        let shared = Arc::clone(state);
        sink.register(name, move |arguments| {
            shared
                .lock()
                .map_or(u64::MAX, |mut state| handler(&mut state, arguments))
        });
    }
}

impl DefinitionServiceState {
    fn ll_float_literal(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, type_id, start, end] = arguments else {
            return u64::MAX;
        };
        let Some(ty @ Type::Float(32 | 64)) = self.kir_type(Some(*type_id)) else {
            return u64::MAX;
        };
        let Some(literal) = self.source_text(Some(start), Some(end)) else {
            return u64::MAX;
        };
        if literal.parse::<f64>().is_err() {
            return u64::MAX;
        }
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        let name = function.builder.const_float(ty.clone(), literal);
        self.kir_new_value(*function_id, name, ty)
    }

    fn ll_float_binary(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, operator, left, right] = arguments else {
            return u64::MAX;
        };
        let operator = match signed(Some(operator)) {
            Some(0) => FloatBinaryOperator::Add,
            Some(1) => FloatBinaryOperator::Sub,
            Some(2) => FloatBinaryOperator::Mul,
            Some(3) => FloatBinaryOperator::Div,
            Some(4) => FloatBinaryOperator::Rem,
            _ => return u64::MAX,
        };
        let (Some(lhs), Some(rhs), Some(ty @ Type::Float(_))) = (
            self.value_name(*function_id, Some(*left)),
            self.value_name(*function_id, Some(*right)),
            self.kir_value_type(*function_id, Some(*left)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        let name = function.builder.float_binary(operator, lhs, rhs);
        self.kir_new_value(*function_id, name, ty)
    }

    fn ll_float_compare(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, predicate, left, right] = arguments else {
            return u64::MAX;
        };
        let predicate = match signed(Some(predicate)) {
            Some(0) => FloatPredicate::Eq,
            Some(1) => FloatPredicate::UnorderedNe,
            Some(2) => FloatPredicate::Lt,
            Some(3) => FloatPredicate::Le,
            Some(4) => FloatPredicate::Gt,
            Some(5) => FloatPredicate::Ge,
            _ => return u64::MAX,
        };
        let (Some(lhs), Some(rhs)) = (
            self.value_name(*function_id, Some(*left)),
            self.value_name(*function_id, Some(*right)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        let name = function.builder.float_compare(predicate, lhs, rhs);
        self.kir_new_value(*function_id, name, Type::Integer(1))
    }

    fn ll_float_negate(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, operand] = arguments else {
            return u64::MAX;
        };
        let (Some(value), Some(ty @ Type::Float(_))) = (
            self.value_name(*function_id, Some(*operand)),
            self.kir_value_type(*function_id, Some(*operand)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        let name = function.builder.float_unary(FloatUnaryOperator::Neg, value);
        self.kir_new_value(*function_id, name, ty)
    }
}
