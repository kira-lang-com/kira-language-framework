use super::{BuilderValue, DefinitionServiceState};
use klf_kir::{BinaryOperator, IntegerPredicate, Type};
use std::sync::{Arc, Mutex};

/// The binary operators offered at both integer widths, keyed by the suffix
/// their service name uses.
static BINARY_OPERATORS: &[(&str, BinaryOperator)] = &[
    ("sub", BinaryOperator::Sub),
    ("mul", BinaryOperator::Mul),
    ("div", BinaryOperator::SignedDiv),
    ("rem", BinaryOperator::SignedRem),
    ("and", BinaryOperator::And),
    ("or", BinaryOperator::Or),
    ("xor", BinaryOperator::Xor),
    ("shl", BinaryOperator::ShiftLeft),
    ("lshr", BinaryOperator::LogicalShiftRight),
    ("ashr", BinaryOperator::ArithmeticShiftRight),
];

/// The signed comparisons, keyed the same way.
static INTEGER_PREDICATES: &[(&str, IntegerPredicate)] = &[
    ("eq", IntegerPredicate::Eq),
    ("ne", IntegerPredicate::Ne),
    ("lt", IntegerPredicate::SignedLt),
    ("le", IntegerPredicate::SignedLe),
    ("gt", IntegerPredicate::SignedGt),
    ("ge", IntegerPredicate::SignedGe),
];

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    capabilities.register_state(
        "klfsvc_kir_value_const_bool",
        state,
        DefinitionServiceState::value_const_bool,
    );
    for width in [64u16, 32] {
        let ty = Type::Integer(width);
        for (suffix, operation) in BINARY_OPERATORS {
            let ty = ty.clone();
            capabilities.register_state(
                format!("klfsvc_kir_value_{suffix}_i{width}"),
                state,
                move |state, arguments| state.value_binary(arguments, *operation, &ty),
            );
        }
        for (suffix, predicate) in INTEGER_PREDICATES {
            capabilities.register_state(
                format!("klfsvc_kir_value_{suffix}_i{width}"),
                state,
                move |state, arguments| state.value_compare(arguments, *predicate),
            );
        }
    }
    capabilities.register_state(
        "klfsvc_kir_value_add_i32",
        state,
        move |state, arguments| {
            state.value_binary(arguments, BinaryOperator::Add, &Type::Integer(32))
        },
    );
    capabilities.register_state(
        "klfsvc_kir_function_conditional_branch",
        state,
        DefinitionServiceState::function_conditional_branch,
    );
    capabilities.register_state(
        "klfsvc_kir_function_conditional_branch_i64",
        state,
        DefinitionServiceState::function_conditional_branch_i64,
    );
    capabilities.register_state(
        "klfsvc_kir_function_conditional_branch_i32",
        state,
        DefinitionServiceState::function_conditional_branch_i32,
    );
}

impl DefinitionServiceState {
    fn value_const_bool(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(raw) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let value = match i64::from_ne_bytes(raw.to_ne_bytes()) {
            0 => false,
            1 => true,
            _ => return u64::MAX,
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
        let name = function.builder.const_bool(value);
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

    fn value_binary(
        &mut self,
        arguments: &[u64],
        operation: klf_kir::BinaryOperator,
        ty: &klf_kir::Type,
    ) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(lhs_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(rhs_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        if self.bool_values.contains(&lhs_id) || self.bool_values.contains(&rhs_id) {
            return u64::MAX;
        }
        let Some(lhs) = self.value_name(function_id, Some(lhs_id)) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, Some(rhs_id)) else {
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
        let name = function
            .builder
            .binary(operation, klf_kir::Overflow::Wrap, true, lhs, rhs);
        let id = u64::try_from(self.values.len()).unwrap_or(u64::MAX);
        if id == u64::MAX {
            return id;
        }
        self.values.push(Some(BuilderValue {
            function: function_id,
            name,
        }));
        if *ty == klf_kir::Type::Integer(32) {
            self.i32_values.insert(id);
        }
        id
    }

    fn value_compare(&mut self, arguments: &[u64], predicate: klf_kir::IntegerPredicate) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(lhs_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        let Some(rhs_id) = arguments.get(2).copied() else {
            return u64::MAX;
        };
        if self.bool_values.contains(&lhs_id) || self.bool_values.contains(&rhs_id) {
            return u64::MAX;
        }
        let Some(lhs) = self.value_name(function_id, Some(lhs_id)) else {
            return u64::MAX;
        };
        let Some(rhs) = self.value_name(function_id, Some(rhs_id)) else {
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
        let name = function.builder.compare(predicate, lhs, rhs);
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

    fn function_conditional_branch(&mut self, arguments: &[u64]) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(condition_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        if !self.bool_values.contains(&condition_id) {
            return u64::MAX;
        }
        let Some(condition) = self.value_name(function_id, Some(condition_id)) else {
            return u64::MAX;
        };
        let Some(then_block) = self.block_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(else_block) = self.block_name(function_id, arguments.get(3).copied()) else {
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
        function.builder.conditional_branch(
            condition,
            then_block,
            Vec::new(),
            else_block,
            Vec::new(),
        );
        0
    }

    fn function_conditional_branch_i64(&mut self, arguments: &[u64]) -> u64 {
        self.function_conditional_branch_typed(arguments, &klf_kir::Type::Integer(64))
    }

    fn function_conditional_branch_i32(&mut self, arguments: &[u64]) -> u64 {
        self.function_conditional_branch_typed(arguments, &klf_kir::Type::Integer(32))
    }

    fn function_conditional_branch_typed(&mut self, arguments: &[u64], ty: &klf_kir::Type) -> u64 {
        let Some(function_id) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some(condition_id) = arguments.get(1).copied() else {
            return u64::MAX;
        };
        if !self.bool_values.contains(&condition_id) {
            return u64::MAX;
        }
        let Some(condition) = self.value_name(function_id, Some(condition_id)) else {
            return u64::MAX;
        };
        let Some(then_block) = self.block_name(function_id, arguments.get(2).copied()) else {
            return u64::MAX;
        };
        let Some(then_value) = self.value_name(function_id, arguments.get(3).copied()) else {
            return u64::MAX;
        };
        let Some(then_value_id) = arguments.get(3).copied() else {
            return u64::MAX;
        };
        let Some(else_block) = self.block_name(function_id, arguments.get(4).copied()) else {
            return u64::MAX;
        };
        let Some(else_value) = self.value_name(function_id, arguments.get(5).copied()) else {
            return u64::MAX;
        };
        let Some(else_value_id) = arguments.get(5).copied() else {
            return u64::MAX;
        };
        if *ty == klf_kir::Type::Integer(32)
            && (!self.i32_values.contains(&then_value_id)
                || !self.i32_values.contains(&else_value_id))
        {
            return u64::MAX;
        }
        if self
            .block_parameter(function_id, arguments.get(2).copied(), ty)
            .is_none()
            || self
                .block_parameter(function_id, arguments.get(4).copied(), ty)
                .is_none()
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
        function.builder.conditional_branch(
            condition,
            then_block,
            vec![then_value],
            else_block,
            vec![else_value],
        );
        0
    }
}
