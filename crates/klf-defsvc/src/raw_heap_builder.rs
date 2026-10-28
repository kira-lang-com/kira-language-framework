//! Unmanaged memory (section 12.9): `heap.alloc.raw` and `heap.free.raw`,
//! through which a language whose std owns an allocator, such as C++'s
//! `malloc` and `operator new`, reaches the KLF runtime; and `mem.copy`, which
//! copies a value owning nothing as its bytes.

use super::DefinitionServiceState;
use klf_kir::Type;
use std::sync::{Arc, Mutex};

mod machine;

pub(super) fn register(
    state: &Arc<Mutex<DefinitionServiceState>>,
    capabilities: &mut crate::ServiceSink,
) {
    machine::register(state, capabilities);
    let shared = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_heap_alloc_raw", move |arguments| {
        shared.lock().map_or(u64::MAX, |mut state| {
            state.ll_value_heap_alloc_raw(arguments)
        })
    });
    let shared = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_heap_free_raw", move |arguments| {
        shared.lock().map_or(u64::MAX, |mut state| {
            state.ll_value_heap_free_raw(arguments)
        })
    });
    let shared = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_mem_copy", move |arguments| {
        shared
            .lock()
            .map_or(u64::MAX, |mut state| state.ll_value_mem_copy(arguments))
    });
    let shared = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_mem_move", move |arguments| {
        shared
            .lock()
            .map_or(u64::MAX, |mut state| state.ll_value_mem_move(arguments))
    });
    let shared = Arc::clone(state);
    capabilities.register("klfsvc_kir_value_mem_set", move |arguments| {
        shared
            .lock()
            .map_or(u64::MAX, |mut state| state.ll_value_mem_set(arguments))
    });
}

impl DefinitionServiceState {
    fn ll_value_heap_alloc_raw(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, size, align] = arguments else {
            return u64::MAX;
        };
        let (Some(size), Some(align)) = (
            self.value_name(*function_id, Some(*size)),
            self.value_name(*function_id, Some(*align)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        let name = function.builder.heap_alloc_raw(size, align);
        self.kir_new_value(*function_id, name, Type::Pointer)
    }

    fn ll_value_heap_free_raw(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, pointer] = arguments else {
            return u64::MAX;
        };
        let Some(pointer) = self.value_name(*function_id, Some(*pointer)) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        function.builder.heap_free_raw(pointer);
        0
    }

    fn ll_value_mem_copy(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, dest, source, length] = arguments else {
            return u64::MAX;
        };
        let (Some(dest), Some(source), Some(length)) = (
            self.value_name(*function_id, Some(*dest)),
            self.value_name(*function_id, Some(*source)),
            self.value_name(*function_id, Some(*length)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        function.builder.mem_copy(dest, source, length);
        0
    }

    fn ll_value_mem_move(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, dest, source, length] = arguments else {
            return u64::MAX;
        };
        let (Some(dest), Some(source), Some(length)) = (
            self.value_name(*function_id, Some(*dest)),
            self.value_name(*function_id, Some(*source)),
            self.value_name(*function_id, Some(*length)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        function.builder.mem_move(dest, source, length);
        0
    }

    fn ll_value_mem_set(&mut self, arguments: &[u64]) -> u64 {
        let [function_id, dest, byte, length] = arguments else {
            return u64::MAX;
        };
        let (Some(dest), Some(byte), Some(length)) = (
            self.value_name(*function_id, Some(*dest)),
            self.value_name(*function_id, Some(*byte)),
            self.value_name(*function_id, Some(*length)),
        ) else {
            return u64::MAX;
        };
        let Some(function) = self.kir_function_mut(*function_id) else {
            return u64::MAX;
        };
        function.builder.mem_set(dest, byte, length);
        0
    }
}
