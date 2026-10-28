//! Declarations of the `klfrt_*` runtime imports, shared by every lowered
//! function. Mirrors `klf-native-rt`'s `extern "C"` surface.

use crate::{EmitError, err};
use cranelift::codegen::ir::{AbiParam, Signature, types};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

pub(crate) struct Runtime {
    pub trap: FuncId,
    /// `klfrt_trap_message`: a trap a language check raised, with its
    /// message.
    pub trap_message: FuncId,
    pub frame_enter: FuncId,
    pub frame_exit: FuncId,
    pub depth_enter: FuncId,
    pub depth_exit: FuncId,
    pub stack_init: FuncId,
    pub stack_alloc: FuncId,
    pub stack_dealloc: FuncId,
    pub heap_alloc_counted: FuncId,
    pub heap_alloc_unique: FuncId,
    pub heap_alloc_untracked: FuncId,
    pub is_unique: FuncId,
    pub global_alloc: FuncId,
    pub retain: FuncId,
    pub release: FuncId,
    pub free_raw: FuncId,
    pub access_begin: FuncId,
    pub access_end: FuncId,
    pub finish: FuncId,
    pub fail: FuncId,
    pub error_box: FuncId,
    pub error_value: FuncId,
    pub process_start: FuncId,
    pub exit: FuncId,
    /// `klfrt_float_remainder`: `fmod` of two doubles, which Cranelift has
    /// no instruction for.
    pub float_remainder: FuncId,
    /// `klfrt_poll` and `_Unwind_Resume`, the slow path of a cancellation
    /// poll and how a cleanup pad continues an unwind, for an object with
    /// unwind tables.
    pub unwinding: Option<(FuncId, FuncId)>,
}

impl Runtime {
    pub(crate) fn declare(module: &mut ObjectModule, unwinding: bool) -> Result<Self, EmitError> {
        let ptr = module.target_config().pointer_type();
        let import = |module: &mut ObjectModule,
                      name: &str,
                      params: &[types::Type],
                      returns: &[types::Type]|
         -> Result<FuncId, EmitError> {
            let mut signature = module.make_signature();
            for ty in params {
                signature.params.push(AbiParam::new(*ty));
            }
            for ty in returns {
                signature.returns.push(AbiParam::new(*ty));
            }
            declare(module, name, &signature)
        };

        Ok(Self {
            trap: import(module, "klfrt_trap", &[types::I32], &[])?,
            trap_message: import(module, "klfrt_trap_message", &[ptr, ptr], &[])?,
            frame_enter: import(module, "klfrt_frame_enter", &[], &[])?,
            frame_exit: import(module, "klfrt_frame_exit", &[], &[])?,
            depth_enter: import(module, "klfrt_depth_enter", &[], &[])?,
            depth_exit: import(module, "klfrt_depth_exit", &[], &[])?,
            stack_init: import(module, "klfrt_stack_init", &[ptr], &[])?,
            stack_alloc: import(
                module,
                "klfrt_stack_alloc",
                &[types::I64, types::I64],
                &[ptr],
            )?,
            stack_dealloc: import(module, "klfrt_stack_dealloc", &[ptr], &[])?,
            heap_alloc_counted: import(
                module,
                "klfrt_heap_alloc_counted",
                &[types::I64, types::I64],
                &[ptr],
            )?,
            heap_alloc_unique: import(
                module,
                "klfrt_heap_alloc_unique",
                &[types::I64, types::I64],
                &[ptr],
            )?,
            heap_alloc_untracked: import(
                module,
                "klfrt_heap_alloc_untracked",
                &[types::I64, types::I64],
                &[ptr],
            )?,
            is_unique: import(module, "klfrt_is_unique", &[ptr], &[types::I8])?,
            global_alloc: import(
                module,
                "klfrt_global_alloc",
                &[types::I64, types::I64, types::I32, types::I64],
                &[ptr],
            )?,
            retain: import(module, "klfrt_retain", &[ptr], &[])?,
            release: import(module, "klfrt_release", &[ptr], &[])?,
            free_raw: import(module, "klfrt_free_raw", &[ptr], &[])?,
            access_begin: import(
                module,
                "klfrt_access_begin",
                &[ptr, types::I32, types::I64],
                &[],
            )?,
            access_end: import(module, "klfrt_access_end", &[ptr], &[])?,
            finish: import(module, "klfrt_finish", &[types::I64, types::I32], &[])?,
            fail: import(module, "klfrt_fail", &[types::I64], &[])?,
            error_box: import(module, "klfrt_error_box", &[types::I64], &[ptr])?,
            error_value: import(module, "klfrt_error_value", &[], &[types::I64])?,
            process_start: import(module, "klfrt_process_start", &[types::I32], &[])?,
            exit: import(module, "klfrt_exit", &[types::I32], &[])?,
            float_remainder: import(
                module,
                "klfrt_float_remainder",
                &[types::F64, types::F64],
                &[types::F64],
            )?,
            unwinding: if unwinding {
                Some((
                    import(module, "klfrt_poll", &[], &[])?,
                    import(module, "_Unwind_Resume", &[ptr], &[])?,
                ))
            } else {
                None
            },
        })
    }

    /// Whether a call of runtime function `id` may raise an interrupt, and
    /// so needs a pad when the frame owns something. Releasing, freeing and
    /// leaving a frame never do.
    pub(crate) fn raises(&self, id: FuncId) -> bool {
        ![
            self.retain,
            self.release,
            self.is_unique,
            self.free_raw,
            self.access_begin,
            self.access_end,
            self.frame_exit,
            self.depth_exit,
            self.stack_dealloc,
            self.float_remainder,
        ]
        .contains(&id)
    }
}

fn declare(
    module: &mut ObjectModule,
    name: &str,
    signature: &Signature,
) -> Result<FuncId, EmitError> {
    module
        .declare_function(name, Linkage::Import, signature)
        .map_err(|e| err(format!("declare runtime `{name}`: {e}")))
}
