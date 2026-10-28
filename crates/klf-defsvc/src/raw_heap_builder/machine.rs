//! The instructions close to the machine, by handle: `select`, `fma`, the
//! raw pointer instructions (section 12.8), the atomics and `fence`, the
//! layout queries (12.16) and the vectors of 12.5. Section 12.14 gives the atomics C11's memory
//! orders, so an order is C11's `memory_order` value; KIR has no consume
//! order, so 1 is refused. An atomic operator is its position in the list
//! of 12.14: xchg, add, sub, and, or, xor, nand, max, min, umax, umin.

use super::super::DefinitionServiceState;
use klf_kir::{AtomicOperator, MemoryOrder, RawAccess, Type};
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
    service!(state, capabilities, "klfsvc_kir_value_select", ll_select);
    service!(state, capabilities, "klfsvc_kir_value_fma", ll_fma);
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_raw_load",
        ll_raw_load
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_raw_store",
        ll_raw_store
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_pointer_offset",
        ll_pointer_offset
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_pointer_diff",
        ll_pointer_diff
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_atomic_load",
        ll_atomic_load
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_atomic_store",
        ll_atomic_store
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_atomic_rmw",
        ll_atomic_rmw
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_compare_exchange",
        ll_compare_exchange
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_compare_exchange_succeeded",
        ll_compare_exchange_succeeded
    );
    service!(state, capabilities, "klfsvc_kir_value_fence", ll_fence);
    service!(state, capabilities, "klfsvc_kir_value_size_of", ll_size_of);
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_align_of",
        ll_align_of
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_stride_of",
        ll_stride_of
    );
    register_vectors(state, capabilities);
}

fn register_vectors(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    service!(
        state,
        capabilities,
        "klfsvc_kir_type_vector",
        ll_type_vector
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_vector_splat",
        ll_vector_splat
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_vector_extract",
        ll_vector_extract
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_vector_insert",
        ll_vector_insert
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_vector_shuffle",
        ll_vector_shuffle
    );
    service!(
        state,
        capabilities,
        "klfsvc_kir_value_vector_shuffle_lane",
        ll_vector_shuffle_lane
    );
}

fn memory_order(code: u64) -> Option<MemoryOrder> {
    Some(match code {
        0 => MemoryOrder::Relaxed,
        2 => MemoryOrder::Acquire,
        3 => MemoryOrder::Release,
        4 => MemoryOrder::AcquireRelease,
        5 => MemoryOrder::SequentiallyConsistent,
        _ => return None,
    })
}

fn atomic_operator(code: u64) -> Option<AtomicOperator> {
    Some(match code {
        0 => AtomicOperator::Exchange,
        1 => AtomicOperator::Add,
        2 => AtomicOperator::Sub,
        3 => AtomicOperator::And,
        4 => AtomicOperator::Or,
        5 => AtomicOperator::Xor,
        6 => AtomicOperator::Nand,
        7 => AtomicOperator::Max,
        8 => AtomicOperator::Min,
        9 => AtomicOperator::UnsignedMax,
        10 => AtomicOperator::UnsignedMin,
        _ => return None,
    })
}

/// An alignment of 0 is the type's own; volatility is 0 or 1.
fn raw_access(align: u64, volatile: u64) -> Option<RawAccess> {
    Some(RawAccess {
        align: match align {
            0 => None,
            align => Some(u32::try_from(align).ok()?),
        },
        volatile: match volatile {
            0 => false,
            1 => true,
            _ => return None,
        },
    })
}

impl DefinitionServiceState {
    /// The function handle that starts `arguments`, the names of the `N`
    /// value handles after it, and the type of the first of them.
    fn operands<const N: usize>(
        &self,
        arguments: &[u64],
    ) -> Option<(u64, [klf_core::Name; N], Type)> {
        let function = *arguments.first()?;
        let mut names = Vec::with_capacity(N);
        for value in arguments.get(1..=N)? {
            names.push(self.value_name(function, Some(*value))?);
        }
        let ty = self.kir_value_type(function, arguments.get(1).copied())?;
        Some((function, names.try_into().ok()?, ty))
    }

    fn ll_select(&mut self, arguments: &[u64]) -> u64 {
        let Some(function) = arguments.first().copied() else {
            return u64::MAX;
        };
        let (Some(condition), Some(then_value), Some(else_value), Some(ty)) = (
            self.value_name(function, arguments.get(1).copied()),
            self.value_name(function, arguments.get(2).copied()),
            self.value_name(function, arguments.get(3).copied()),
            self.kir_value_type(function, arguments.get(2).copied()),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.select(condition, then_value, else_value);
        self.kir_new_value(function, name, ty)
    }

    fn ll_fma(&mut self, arguments: &[u64]) -> u64 {
        let Some((function, [first, second, addend], ty)) = self.operands::<3>(arguments) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.fma(first, second, addend);
        self.kir_new_value(function, name, ty)
    }

    fn ll_raw_load(&mut self, arguments: &[u64]) -> u64 {
        let [function, ty, pointer, align, volatile] = *arguments else {
            return u64::MAX;
        };
        let (Some(ty), Some(pointer), Some(access)) = (
            self.kir_type(Some(ty)),
            self.value_name(function, Some(pointer)),
            raw_access(align, volatile),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.raw_load(ty.clone(), pointer, access);
        self.kir_new_value(function, name, ty)
    }

    fn ll_raw_store(&mut self, arguments: &[u64]) -> u64 {
        let [function, value, pointer, align, volatile] = *arguments else {
            return u64::MAX;
        };
        let (Some(value), Some(pointer), Some(access)) = (
            self.value_name(function, Some(value)),
            self.value_name(function, Some(pointer)),
            raw_access(align, volatile),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        builder.builder.raw_store(value, pointer, access);
        0
    }

    fn ll_pointer_offset(&mut self, arguments: &[u64]) -> u64 {
        let Some((function, [pointer, bytes], ty)) = self.operands::<2>(arguments) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.pointer_offset(pointer, bytes);
        self.kir_new_value(function, name, ty)
    }

    fn ll_pointer_diff(&mut self, arguments: &[u64]) -> u64 {
        let Some((function, [lhs, rhs], _)) = self.operands::<2>(arguments) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.pointer_diff(lhs, rhs);
        self.kir_new_value(function, name, Type::Integer(64))
    }

    fn ll_atomic_load(&mut self, arguments: &[u64]) -> u64 {
        let [function, ty, pointer, order] = *arguments else {
            return u64::MAX;
        };
        let (Some(ty), Some(pointer), Some(order)) = (
            self.kir_type(Some(ty)),
            self.value_name(function, Some(pointer)),
            memory_order(order),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.atomic_load(ty.clone(), pointer, order);
        self.kir_new_value(function, name, ty)
    }

    fn ll_atomic_store(&mut self, arguments: &[u64]) -> u64 {
        let [function, value, pointer, order] = *arguments else {
            return u64::MAX;
        };
        let (Some(value), Some(pointer), Some(order)) = (
            self.value_name(function, Some(value)),
            self.value_name(function, Some(pointer)),
            memory_order(order),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        builder.builder.atomic_store(value, pointer, order);
        0
    }

    fn ll_atomic_rmw(&mut self, arguments: &[u64]) -> u64 {
        let [function, operator, pointer, value, order] = *arguments else {
            return u64::MAX;
        };
        let (Some(operator), Some(pointer), Some(name), Some(ty), Some(order)) = (
            atomic_operator(operator),
            self.value_name(function, Some(pointer)),
            self.value_name(function, Some(value)),
            self.kir_value_type(function, Some(value)),
            memory_order(order),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let result = builder.builder.atomic_rmw(operator, pointer, name, order);
        self.kir_new_value(function, result, ty)
    }

    /// Answers the value read; `compare_exchange_succeeded` answers whether
    /// the exchange happened.
    fn ll_compare_exchange(&mut self, arguments: &[u64]) -> u64 {
        let [
            function,
            weak,
            pointer,
            expected,
            replacement,
            success,
            failure,
        ] = *arguments
        else {
            return u64::MAX;
        };
        let (Some(pointer), Some(expected_name), Some(replacement), Some(ty)) = (
            self.value_name(function, Some(pointer)),
            self.value_name(function, Some(expected)),
            self.value_name(function, Some(replacement)),
            self.kir_value_type(function, Some(expected)),
        ) else {
            return u64::MAX;
        };
        let (Some(success), Some(failure), true) =
            (memory_order(success), memory_order(failure), weak <= 1)
        else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let (read, succeeded) = builder.builder.compare_exchange(
            weak == 1,
            pointer,
            expected_name,
            replacement,
            (success, failure),
        );
        let read = self.kir_new_value(function, read, ty);
        let succeeded = self.kir_new_value(function, succeeded, Type::Integer(1));
        self.kir_second_results.insert((function, read), succeeded);
        read
    }

    fn ll_compare_exchange_succeeded(&mut self, arguments: &[u64]) -> u64 {
        let [function, read] = *arguments else {
            return u64::MAX;
        };
        self.kir_second_results
            .get(&(function, read))
            .copied()
            .unwrap_or(u64::MAX)
    }

    fn ll_fence(&mut self, arguments: &[u64]) -> u64 {
        let [function, order] = *arguments else {
            return u64::MAX;
        };
        let Some(order) = memory_order(order) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        builder.builder.fence(order);
        0
    }

    fn ll_size_of(&mut self, arguments: &[u64]) -> u64 {
        self.layout_query(arguments, klf_services::FunctionBuilder::type_size)
    }

    fn ll_align_of(&mut self, arguments: &[u64]) -> u64 {
        self.layout_query(arguments, klf_services::FunctionBuilder::type_align)
    }

    fn ll_stride_of(&mut self, arguments: &[u64]) -> u64 {
        self.layout_query(arguments, klf_services::FunctionBuilder::type_stride)
    }

    fn layout_query(
        &mut self,
        arguments: &[u64],
        query: fn(&mut klf_services::FunctionBuilder, Type) -> klf_core::Name,
    ) -> u64 {
        let [function, ty] = *arguments else {
            return u64::MAX;
        };
        let Some(ty) = self.kir_type(Some(ty)) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = query(&mut builder.builder, ty);
        self.kir_new_value(function, name, Type::Integer(64))
    }

    fn ll_type_vector(&mut self, arguments: &[u64]) -> u64 {
        let [lanes, element] = *arguments else {
            return u64::MAX;
        };
        let (Ok(lanes), Some(element)) = (u32::try_from(lanes), self.kir_type(Some(element)))
        else {
            return u64::MAX;
        };
        self.kir_new_type(Type::Vector(lanes, Box::new(element)))
    }

    fn ll_vector_splat(&mut self, arguments: &[u64]) -> u64 {
        let [function, ty, value] = *arguments else {
            return u64::MAX;
        };
        let (Some(ty), Some(value)) = (
            self.kir_type(Some(ty)),
            self.value_name(function, Some(value)),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.vector_splat(ty.clone(), value);
        self.kir_new_value(function, name, ty)
    }

    /// The name, lane count and element type of a vector value handle.
    fn vector(&self, function: u64, value: u64) -> Option<(klf_core::Name, u32, Type)> {
        let name = self.value_name(function, Some(value))?;
        match self.kir_value_type(function, Some(value))? {
            Type::Vector(lanes, element) => Some((name, lanes, *element)),
            _ => None,
        }
    }

    fn ll_vector_extract(&mut self, arguments: &[u64]) -> u64 {
        let [function, vector, lane] = *arguments else {
            return u64::MAX;
        };
        let (Some((vector, _, element)), Ok(lane)) =
            (self.vector(function, vector), u32::try_from(lane))
        else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.vector_extract(vector, lane);
        self.kir_new_value(function, name, element)
    }

    fn ll_vector_insert(&mut self, arguments: &[u64]) -> u64 {
        let [function, vector, lane, value] = *arguments else {
            return u64::MAX;
        };
        let (Some((vector, lanes, element)), Ok(lane), Some(value)) = (
            self.vector(function, vector),
            u32::try_from(lane),
            self.value_name(function, Some(value)),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.vector_insert(vector, lane, value);
        self.kir_new_value(function, name, Type::Vector(lanes, Box::new(element)))
    }

    /// A shuffle of one lane; `vector_shuffle_lane` adds each further lane
    /// before anything uses the result.
    fn ll_vector_shuffle(&mut self, arguments: &[u64]) -> u64 {
        let [function, lhs, rhs, lane] = *arguments else {
            return u64::MAX;
        };
        let (Some((lhs, _, element)), Some(rhs), Ok(lane)) = (
            self.vector(function, lhs),
            self.value_name(function, Some(rhs)),
            u32::try_from(lane),
        ) else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        let name = builder.builder.vector_shuffle(lhs, rhs, vec![lane]);
        self.kir_new_value(function, name, Type::Vector(1, Box::new(element)))
    }

    fn ll_vector_shuffle_lane(&mut self, arguments: &[u64]) -> u64 {
        let [function, shuffle, lane] = *arguments else {
            return u64::MAX;
        };
        let (Some((name, lanes, element)), Ok(lane)) =
            (self.vector(function, shuffle), u32::try_from(lane))
        else {
            return u64::MAX;
        };
        let Some(builder) = self.kir_function_mut(function) else {
            return u64::MAX;
        };
        if !builder.builder.extend_shuffle(&name, lane) {
            return u64::MAX;
        }
        self.set_value_type(
            shuffle,
            function,
            Type::Vector(lanes + 1, Box::new(element)),
        );
        0
    }
}
