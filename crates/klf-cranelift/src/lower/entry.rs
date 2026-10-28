//! The program entry the backend synthesizes around a lowered module.
//!
//! A plain native run gets a `main` that allocates the module's globals, calls
//! one entry function with fixed arguments and reports the outcome through
//! `klfrt_finish`. Each definition of a native frontend gets the same global
//! prologue and a trampoline the runtime calls the `Language` phases
//! through; the registry gives the binary its `main`.

use super::{GlobalMap, IdMap, TRAP, arith, cl_type};
use crate::runtime::Runtime;
use crate::unwind::UnwindTables;
use crate::{EmitError, EntryConvention, err, is_void};
use cranelift::codegen::Context;
use cranelift::codegen::ir::{MemFlagsData, StackSlotData, StackSlotKind};
use cranelift::prelude::{
    AbiParam, FunctionBuilder, FunctionBuilderContext, InstBuilder, Value, types,
};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;
use klf_kir::{Module as KirModule, Type};

/// The function a program's `main` calls, what it passes, and how the
/// program ends.
pub(crate) struct ProgramEntry<'e> {
    pub(crate) name: &'e str,
    pub(crate) arguments: &'e [u64],
    pub(crate) convention: EntryConvention,
}

pub(crate) fn emit_main(
    module: &mut ObjectModule,
    kir: &KirModule,
    program: &ProgramEntry<'_>,
    runtime: &Runtime,
    ids: &IdMap,
    globals: &GlobalMap,
) -> Result<(), EmitError> {
    let ProgramEntry {
        name: entry,
        arguments,
        convention,
    } = *program;
    let function = kir
        .functions
        .iter()
        .find(|function| function.name.as_str() == entry)
        .ok_or_else(|| err(format!("no function named `{entry}`")))?;
    let entry_id = ids
        .get(entry)
        .map(|(id, _)| *id)
        .ok_or_else(|| err("entry not declared"))?;

    let process = matches!(
        convention,
        EntryConvention::Process | EntryConvention::ProcessExit
    );
    let pointer = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    if process {
        signature.params.push(AbiParam::new(types::I32));
        signature.params.push(AbiParam::new(pointer));
        signature.params.push(AbiParam::new(pointer));
    }
    signature.returns.push(AbiParam::new(types::I32));
    let main_id = module
        .declare_function("main", Linkage::Export, &signature)
        .map_err(|e| err(format!("declare main: {e}")))?;

    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        if process {
            builder.append_block_params_for_function_params(block);
            let start = module.declare_func_in_func(runtime.process_start, builder.func);
            let trap_exits = builder.ins().iconst(
                types::I32,
                i64::from(convention == EntryConvention::ProcessExit),
            );
            builder.ins().call(start, &[trap_exits]);
        }

        emit_global_prologue(module, &mut builder, kir, runtime, globals, true)?;
        emit_initializer_calls(module, &mut builder, kir, ids)?;

        let mut args = Vec::new();
        if process {
            let incoming = builder.block_params(block).to_vec();
            for (parameter, value) in function.parameters.iter().zip(incoming) {
                args.push(process_argument(&mut builder, &parameter.ty, value)?);
            }
        } else {
            for (index, parameter) in function.parameters.iter().enumerate() {
                let value = arguments.get(index).copied().unwrap_or(0);
                args.push(main_argument(&mut builder, &parameter.ty, value)?);
            }
        }
        let results = call_kir(
            module,
            &mut builder,
            runtime,
            entry_id,
            &args,
            function.failure.is_some(),
        );

        if process {
            // C semantics: the entry's value, truncated to `int`, is the exit status.
            let status = if is_void(&function.result) {
                builder.ins().iconst(types::I32, 0)
            } else {
                exit_status(&mut builder, &function.result, results[0])?
            };
            let exit = module.declare_func_in_func(runtime.exit, builder.func);
            builder.ins().call(exit, &[status]);
            builder.ins().trap(TRAP);
            let config = module.target_config();
            builder.finalize(config);
        } else {
            let finish = module.declare_func_in_func(runtime.finish, builder.func);
            if is_void(&function.result) {
                let zero = builder.ins().iconst(types::I64, 0);
                let has = builder.ins().iconst(types::I32, 0);
                builder.ins().call(finish, &[zero, has]);
            } else {
                let widened = widen_result(&mut builder, &function.result, results[0])?;
                let has = builder.ins().iconst(types::I32, 1);
                builder.ins().call(finish, &[widened, has]);
            }
            builder.ins().trap(TRAP);
            let config = module.target_config();
            builder.finalize(config);
        }
    }
    module
        .define_function(main_id, &mut context)
        .map_err(|e| err(format!("define main: {e}")))?;
    Ok(())
}

/// An exported `void init(void)` that runs the global prologue, so a library
/// that defines globals can be initialized before its exports are called.
pub(crate) fn emit_library_init(
    module: &mut ObjectModule,
    kir: &KirModule,
    symbol: &str,
    runtime: &Runtime,
    ids: &IdMap,
    globals: &GlobalMap,
) -> Result<(), EmitError> {
    let signature = module.make_signature();
    let id = module
        .declare_function(symbol, Linkage::Export, &signature)
        .map_err(|e| err(format!("declare {symbol}: {e}")))?;
    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        emit_global_prologue(module, &mut builder, kir, runtime, globals, false)?;
        emit_initializer_calls(module, &mut builder, kir, ids)?;
        builder.ins().return_(&[]);
        let config = module.target_config();
        builder.finalize(config);
    }
    module
        .define_function(id, &mut context)
        .map_err(|e| err(format!("define {symbol}: {e}")))
}

/// Calls the module's `initializer` functions in order, after its globals
/// exist and before anything else runs.
fn emit_initializer_calls(
    module: &mut ObjectModule,
    builder: &mut FunctionBuilder,
    kir: &KirModule,
    ids: &IdMap,
) -> Result<(), EmitError> {
    for function in &kir.functions {
        if function.foreign
            || !function
                .attributes
                .contains(&klf_kir::FunctionAttribute::Initializer)
        {
            continue;
        }
        let id = ids
            .get(function.name.as_str())
            .map(|(id, _)| *id)
            .ok_or_else(|| err(format!("initializer `{}` not declared", function.name)))?;
        let fref = module.declare_func_in_func(id, builder.func);
        builder.ins().call(fref, &[]);
    }
    Ok(())
}

fn main_argument(builder: &mut FunctionBuilder, ty: &Type, value: u64) -> Result<Value, EmitError> {
    Ok(match ty {
        Type::Integer(_) | Type::Pointer => {
            let cl = cl_type(ty)?;
            builder.ins().iconst(cl, arith::imm64(i128::from(value)))
        }
        Type::Float(64) => builder.ins().f64const(f64::from_bits(value)),
        Type::Float(32) => {
            let low = u32::try_from(value & 0xFFFF_FFFF).expect("masked");
            builder.ins().f32const(f32::from_bits(low))
        }
        other => return Err(err(format!("entry argument `{other}` unsupported"))),
    })
}

/// `argc` or `argv` of the C `main`, as the entry's parameter of type `ty`.
fn process_argument(
    builder: &mut FunctionBuilder,
    ty: &Type,
    value: Value,
) -> Result<Value, EmitError> {
    let target = cl_type(ty)?;
    let source = builder.func.dfg.value_type(value);
    Ok(if target == source {
        value
    } else if target.bits() > source.bits() {
        builder.ins().sextend(target, value)
    } else {
        builder.ins().ireduce(target, value)
    })
}

/// An integer entry result as the `int` exit status.
fn exit_status(builder: &mut FunctionBuilder, ty: &Type, value: Value) -> Result<Value, EmitError> {
    Ok(match ty {
        Type::Integer(32) => value,
        Type::Integer(bits) if *bits > 32 => builder.ins().ireduce(types::I32, value),
        Type::Integer(_) => builder.ins().uextend(types::I32, value),
        other => return Err(err(format!("entry returning `{other}` has no exit status"))),
    })
}

/// `argc` or `argv` of the C `main`, as the entry's parameter of type `ty`.
fn widen_result(
    builder: &mut FunctionBuilder,
    ty: &Type,
    value: Value,
) -> Result<Value, EmitError> {
    Ok(match ty {
        Type::Integer(64)
        | Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_) => value,
        Type::Integer(_) => builder.ins().uextend(types::I64, value),
        Type::Float(64) => builder
            .ins()
            .bitcast(types::I64, MemFlagsData::new(), value),
        Type::Float(32) => {
            let bits = builder
                .ins()
                .bitcast(types::I32, MemFlagsData::new(), value);
            builder.ins().uextend(types::I64, bits)
        }
        other => return Err(err(format!("cannot return `{other}` from entry"))),
    })
}

/// Allocates and initializes every module-level global, mirroring the VM: one
/// untracked heap block per global, zeroed, its initializer bytes stored in,
/// and its address written into the module's pointer slot.
fn emit_global_prologue(
    module: &mut ObjectModule,
    builder: &mut FunctionBuilder,
    kir: &KirModule,
    runtime: &Runtime,
    globals: &GlobalMap,
    runs_program: bool,
) -> Result<(), EmitError> {
    // The recursion limit comes from where the stack starts. A function that
    // goes on to run the program gives its own stack pointer too, which every
    // program frame lies below; a library's `init` returns before its host
    // calls in from wherever it likes, so it gives none.
    let init = module.declare_func_in_func(runtime.stack_init, builder.func);
    let pointer = module.target_config().pointer_type();
    let top = if runs_program {
        builder.ins().get_stack_pointer(pointer)
    } else {
        builder.ins().iconst(pointer, 0)
    };
    builder.ins().call(init, &[top]);
    if kir.globals.is_empty() {
        return Ok(());
    }
    // Each global is a row of a table the prologue walks: its size, its
    // alignment, the image of its initial bytes and how long the image is,
    // and the slot that holds its address. A program has thousands of
    // globals, and a call per word of each would make the prologue the
    // largest function of the program, compiled again by every build.
    let Some((table, rows)) = global_table(module, kir, globals)? else {
        return Ok(());
    };
    let table = module.declare_data_in_func(table, builder.func);
    let base = builder.ins().symbol_value(types::I64, table);
    let count = builder
        .ins()
        .iconst(types::I64, i64::try_from(rows).unwrap_or(i64::MAX));
    let header = builder.create_block();
    let body = builder.create_block();
    let copy = builder.create_block();
    let after = builder.create_block();
    let done = builder.create_block();
    builder.append_block_param(header, types::I64);
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().jump(header, &[zero.into()]);

    builder.switch_to_block(header);
    let index = builder.block_params(header)[0];
    let more = builder
        .ins()
        .icmp(cranelift::prelude::IntCC::UnsignedLessThan, index, count);
    builder.ins().brif(more, body, &[], done, &[]);

    builder.switch_to_block(body);
    builder.seal_block(body);
    let offset = builder.ins().imul_imm_u(index, GLOBAL_ROW);
    let row = builder.ins().iadd(base, offset);
    let word = |builder: &mut FunctionBuilder, field: i32| {
        builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), row, field * 8)
    };
    let size = word(builder, 0);
    let align = word(builder, 1);
    let image = word(builder, 2);
    let length = word(builder, 3);
    let slot = word(builder, 4);
    let no_init = builder.ins().iconst(types::I32, 0);
    let alloc = module.declare_func_in_func(runtime.global_alloc, builder.func);
    let call = builder.ins().call(alloc, &[size, align, no_init, zero]);
    let allocated = builder.inst_results(call)[0];
    builder.ins().brif(length, copy, &[], after, &[]);

    builder.switch_to_block(copy);
    builder.seal_block(copy);
    let config = module.target_config();
    builder.call_memcpy(config, allocated, image, length);
    builder.ins().jump(after, &[]);

    builder.switch_to_block(after);
    builder.seal_block(after);
    builder
        .ins()
        .store(MemFlagsData::trusted(), allocated, slot, 0);
    let next = builder.ins().iadd_imm_u(index, 1);
    builder.ins().jump(header, &[next.into()]);
    builder.seal_block(header);

    builder.switch_to_block(done);
    builder.seal_block(done);
    Ok(())
}

/// The bytes of one row of the globals table.
const GLOBAL_ROW: i64 = 40;

/// The table [`emit_global_prologue`] walks, one row per global of `kir`,
/// and the images of their initial bytes it points at, with its row count;
/// nothing when every global is thread-local data, which needs no
/// allocation. A global's memory starts zeroed, so an image stops at its
/// last byte that is not zero.
fn global_table(
    module: &mut ObjectModule,
    kir: &KirModule,
    globals: &GlobalMap,
) -> Result<Option<(cranelift_module::DataId, usize)>, EmitError> {
    let layouts = klf_layout::Layouts::new(&kir.types);
    let mut rows = Vec::with_capacity(kir.globals.len());
    let mut images = Vec::new();
    for global in kir.globals.iter().filter(|global| !global.thread_local) {
        let layout = layouts.layout_of(&global.ty);
        let size = layout.size.max(1);
        let mut bytes = global
            .init
            .init_bytes(usize::try_from(size).unwrap_or(usize::MAX));
        let used = bytes.iter().rposition(|byte| *byte != 0).map_or(0, |last| last + 1);
        bytes.truncate(used);
        let start = images.len();
        images.extend_from_slice(&bytes);
        images.resize(images.len().next_multiple_of(8), 0);
        let slot = *globals
            .get(global.name.as_str())
            .ok_or_else(|| err("missing global slot"))?;
        rows.push((size, layout.align.max(1), start, used, slot));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    let count = rows.len();
    let image = (!images.is_empty())
        .then(|| -> Result<_, EmitError> {
            let id = module
                .declare_anonymous_data(false, false)
                .map_err(|e| err(format!("declare global images: {e}")))?;
            let mut description = DataDescription::new();
            description.set_align(8);
            description.define(images.into_boxed_slice());
            module
                .define_data(id, &description)
                .map_err(|e| err(format!("define global images: {e}")))?;
            Ok(id)
        })
        .transpose()?;
    let table = module
        .declare_anonymous_data(false, false)
        .map_err(|e| err(format!("declare globals table: {e}")))?;
    let mut description = DataDescription::new();
    description.set_align(8);
    let row_bytes = usize::try_from(GLOBAL_ROW).unwrap_or(40);
    let mut contents = vec![0u8; rows.len() * row_bytes];
    let image = image.map(|image| module.declare_data_in_data(image, &mut description));
    for (position, (size, align, start, used, slot)) in rows.into_iter().enumerate() {
        let row = position * row_bytes;
        contents[row..row + 8].copy_from_slice(&size.to_le_bytes());
        contents[row + 8..row + 16].copy_from_slice(&align.to_le_bytes());
        contents[row + 24..row + 32].copy_from_slice(&(used as u64).to_le_bytes());
        let offset = |field: usize| u32::try_from(row + field * 8).unwrap_or(u32::MAX);
        if used > 0
            && let Some(image) = image
        {
            description.write_data_addr(offset(2), image, i64::try_from(start).unwrap_or(0));
        }
        let slot = module.declare_data_in_data(slot, &mut description);
        description.write_data_addr(offset(4), slot, 0);
    }
    description.define(contents.into_boxed_slice());
    module
        .define_data(table, &description)
        .map_err(|e| err(format!("define globals table: {e}")))?;
    Ok(Some((table, count)))
}

/// The four `Language` phase symbols, in the order `klf-services` numbers them.
const PHASES: [&str; 4] = ["units", "parse", "elaborate", "lower"];

/// The KIR entries KLF Kira generates around a `compatDriver` override, a
/// `runProgram` hook, a shader `emit` hook, a `format` hook, the
/// definition's `ParsableCommand`s and a `project` hook, each with the trampoline
/// selector that reaches it and how many argument words it takes. A
/// definition without the member has no such function and its selector
/// answers -1.
const DRIVER_ENTRIES: [(&str, i64, usize); 6] = [
    ("__klf_compat_driver", 4, 1),
    ("__klf_run_program", 5, 2),
    ("__klf_shader_emit", 6, 1),
    ("__klf_format", 7, 3),
    ("__klf_command", 8, 1),
    ("__klf_project", 9, 2),
];

/// Emits the three symbols one definition of a native frontend binary
/// carries on top of its own code, numbered by its `index` in the binary:
/// `klffe_metadata_N` (the definition's declared name, driver, extensions and
/// flags, as a NUL-terminated blob), `klffe_phase_N` (a trampoline from a
/// phase number to the phase function) and `klffe_init_N` (the global
/// prologue). The registry of [`super::registry`] joins them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_frontend_entry(
    module: &mut ObjectModule,
    kir: &KirModule,
    metadata: &str,
    index: usize,
    runtime: &Runtime,
    ids: &IdMap,
    globals: &GlobalMap,
    mut tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    emit_metadata(module, metadata, index, tables.as_deref_mut())?;
    emit_phase_trampoline(module, kir, index, runtime, ids, tables.as_deref_mut())?;
    emit_frontend_init(module, kir, index, runtime, globals, tables)
}

/// Defines `id` from `context`, with unwind tables when `tables` is given.
pub(crate) fn define(
    module: &mut ObjectModule,
    id: FuncId,
    context: &mut Context,
    tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    match tables {
        Some(tables) => tables.define(module, id, context),
        None => module
            .define_function(id, context)
            .map_err(|e| err(e.to_string())),
    }
}

fn emit_metadata(
    module: &mut ObjectModule,
    metadata: &str,
    index: usize,
    tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    let data_id = module
        .declare_data("klffe_metadata_text", Linkage::Local, false, false)
        .map_err(|e| err(format!("declare metadata: {e}")))?;
    let mut description = DataDescription::new();
    let mut bytes = metadata.as_bytes().to_vec();
    bytes.push(0);
    description.define(bytes.into_boxed_slice());
    module
        .define_data(data_id, &description)
        .map_err(|e| err(format!("define metadata: {e}")))?;

    let pointer = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.returns.push(AbiParam::new(pointer));
    let id = module
        .declare_function(
            &format!("klffe_metadata_{index}"),
            Linkage::Export,
            &signature,
        )
        .map_err(|e| err(format!("declare klffe_metadata: {e}")))?;
    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        let value = module.declare_data_in_func(data_id, builder.func);
        let address = builder.ins().symbol_value(pointer, value);
        builder.ins().return_(&[address]);
        let config = module.target_config();
        builder.finalize(config);
    }
    define(module, id, &mut context, tables).map_err(|e| err(format!("define klffe_metadata: {e}")))
}

#[allow(clippy::too_many_lines)]
fn emit_phase_trampoline(
    module: &mut ObjectModule,
    kir: &KirModule,
    index: usize,
    runtime: &Runtime,
    ids: &IdMap,
    tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    let mut signature = module.make_signature();
    for _ in 0..4 {
        signature.params.push(AbiParam::new(types::I64));
    }
    signature.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(&format!("klffe_phase_{index}"), Linkage::Export, &signature)
        .map_err(|e| err(format!("declare klffe_phase: {e}")))?;

    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let selector = builder.block_params(entry)[0];
        let arguments: Vec<_> = builder.block_params(entry)[1..].to_vec();

        let mut current = entry;
        for (index, phase) in PHASES.iter().enumerate() {
            let function = kir
                .functions
                .iter()
                .find(|function| function.name.as_str() == *phase)
                .ok_or_else(|| err(format!("definition has no `{phase}` phase")))?;
            if is_void(&function.result) {
                return Err(err(format!("phase `{phase}` returns no handle")));
            }
            let (phase_id, _) = ids
                .get(*phase)
                .ok_or_else(|| err(format!("phase `{phase}` is not declared")))?;
            let call_block = builder.create_block();
            let next = builder.create_block();
            builder.switch_to_block(current);
            let matched = builder.ins().icmp_imm_s(
                cranelift::prelude::IntCC::Equal,
                selector,
                i64::try_from(index).unwrap_or(-1),
            );
            builder.ins().brif(matched, call_block, &[], next, &[]);
            builder.seal_block(call_block);
            builder.seal_block(next);

            builder.switch_to_block(call_block);
            let passed = &arguments[..function.parameters.len().min(arguments.len())];
            let results = call_kir(
                module,
                &mut builder,
                runtime,
                *phase_id,
                passed,
                function.failure.is_some(),
            );
            let widened = widen_result(&mut builder, &function.result, results[0])?;
            builder.ins().return_(&[widened]);
            current = next;
        }
        for (entry, entry_selector, words) in DRIVER_ENTRIES {
            let Some((entry_id, _)) = kir
                .functions
                .iter()
                .any(|function| function.name.as_str() == entry)
                .then(|| ids.get(entry))
                .flatten()
            else {
                continue;
            };
            let call_block = builder.create_block();
            let next = builder.create_block();
            builder.switch_to_block(current);
            let matched = builder.ins().icmp_imm_s(
                cranelift::prelude::IntCC::Equal,
                selector,
                entry_selector,
            );
            builder.ins().brif(matched, call_block, &[], next, &[]);
            builder.seal_block(call_block);
            builder.seal_block(next);
            builder.switch_to_block(call_block);
            let fails = kir
                .functions
                .iter()
                .any(|function| function.name.as_str() == entry && function.failure.is_some());
            let results = call_kir(
                module,
                &mut builder,
                runtime,
                *entry_id,
                &arguments[..words],
                fails,
            );
            builder.ins().return_(&[results[0]]);
            current = next;
        }
        builder.switch_to_block(current);
        let bad = builder.ins().iconst(types::I64, -1);
        builder.ins().return_(&[bad]);
        let config = module.target_config();
        builder.finalize(config);
    }
    define(module, id, &mut context, tables).map_err(|e| err(format!("define klffe_phase: {e}")))
}

// The registry's `main` gives the runtime the stack top once every
// definition's globals are in place, so no prologue here gives one.
fn emit_frontend_init(
    module: &mut ObjectModule,
    kir: &KirModule,
    index: usize,
    runtime: &Runtime,
    globals: &GlobalMap,
    tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    let signature = module.make_signature();
    let init_id = module
        .declare_function(&format!("klffe_init_{index}"), Linkage::Export, &signature)
        .map_err(|e| err(format!("declare klffe_init: {e}")))?;
    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        emit_global_prologue(module, &mut builder, kir, runtime, globals, false)?;
        builder.ins().return_(&[]);
        let config = module.target_config();
        builder.finalize(config);
    }
    define(module, init_id, &mut context, tables)
        .map_err(|e| err(format!("define klffe_init: {e}")))
}

/// Calls a KIR function from a synthesized wrapper. A function that can
/// `fail` takes the error out-pointer (D-118); a failure that reaches the
/// wrapper ends the process through `klfrt_fail`, as on the LLVM backend.
fn call_kir(
    module: &mut ObjectModule,
    builder: &mut FunctionBuilder,
    runtime: &Runtime,
    callee: FuncId,
    arguments: &[Value],
    fails: bool,
) -> Vec<Value> {
    let fref = module.declare_func_in_func(callee, builder.func);
    if !fails {
        let call = builder.ins().call(fref, arguments);
        return builder.inst_results(call).to_vec();
    }
    let pointer = module.target_config().pointer_type();
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let address = builder.ins().stack_addr(pointer, slot, 0);
    let null = builder.ins().iconst(pointer, 0);
    builder
        .ins()
        .store(MemFlagsData::trusted(), null, address, 0);
    let mut passed = arguments.to_vec();
    passed.push(address);
    let call = builder.ins().call(fref, &passed);
    let results = builder.inst_results(call).to_vec();
    let error = builder
        .ins()
        .load(pointer, MemFlagsData::trusted(), address, 0);
    let failed = builder.create_block();
    let done = builder.create_block();
    builder.ins().brif(error, failed, &[], done, &[]);
    builder.seal_block(failed);
    builder.seal_block(done);
    builder.switch_to_block(failed);
    let error_value = module.declare_func_in_func(runtime.error_value, builder.func);
    let call = builder.ins().call(error_value, &[]);
    let value = builder.inst_results(call)[0];
    let fail = module.declare_func_in_func(runtime.fail, builder.func);
    builder.ins().call(fail, &[value]);
    builder.ins().trap(TRAP);
    builder.switch_to_block(done);
    results
}
