use super::*;

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
pub(super) fn call_kir(
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
