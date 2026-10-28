//! The registry object of a native frontend binary (`KLF-KSL.md` section
//! 5.9). The binary holds one or more definitions, each exporting
//! `klffe_metadata_N`, `klffe_phase_N` and `klffe_init_N`. The runtime calls
//! `klffe_metadata` and `klffe_phase` without knowing how many there are, so
//! the registry answers those for the definition `klffe_select` chose, and
//! gives the binary its `main`.

use crate::unwind::UnwindTables;
use crate::{EmitError, err};
use cranelift::codegen::Context;
use cranelift::codegen::ir::{FuncRef, MemFlagsData};
use cranelift::prelude::{
    AbiParam, FunctionBuilder, FunctionBuilderContext, InstBuilder, IntCC, Signature, Value, types,
};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

/// Emits the registry for `count` definitions.
#[allow(clippy::too_many_lines)]
pub(crate) fn emit_registry(
    module: &mut ObjectModule,
    count: usize,
    mut tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    let pointer = module.target_config().pointer_type();
    let selected = module
        .declare_data("klffe_selected", Linkage::Local, true, false)
        .map_err(|e| err(format!("declare klffe_selected: {e}")))?;
    let mut description = DataDescription::new();
    description.define_zeroinit(8);
    module
        .define_data(selected, &description)
        .map_err(|e| err(format!("define klffe_selected: {e}")))?;

    let metadata_signature = signature(module, &[], &[pointer]);
    let phase_signature = signature(module, &[types::I64; 4], &[types::I64]);
    let init_signature = signature(module, &[], &[]);
    let mut metadata = Vec::new();
    let mut phases = Vec::new();
    let mut inits = Vec::new();
    for index in 0..count {
        metadata.push(import(
            module,
            &format!("klffe_metadata_{index}"),
            &metadata_signature,
        )?);
        phases.push(import(
            module,
            &format!("klffe_phase_{index}"),
            &phase_signature,
        )?);
        inits.push(import(
            module,
            &format!("klffe_init_{index}"),
            &init_signature,
        )?);
    }

    let count_word = i64::try_from(count).unwrap_or(i64::MAX);
    define(
        module,
        tables.as_deref_mut(),
        "klffe_language_count",
        &[],
        &[types::I64],
        |builder, _, _| vec![builder.ins().iconst(types::I64, count_word)],
    )?;
    define(
        module,
        tables.as_deref_mut(),
        "klffe_language_metadata",
        &[types::I64],
        &[pointer],
        |builder, module, parameters| {
            let null = builder.ins().iconst(pointer, 0);
            vec![dispatch(
                builder,
                module,
                parameters[0],
                &metadata,
                &[],
                null,
            )]
        },
    )?;
    define(
        module,
        tables.as_deref_mut(),
        "klffe_select",
        &[types::I64],
        &[types::I64],
        |builder, module, parameters| {
            let address = data_address(builder, module, selected, pointer);
            let previous = builder
                .ins()
                .load(types::I64, MemFlagsData::new(), address, 0);
            builder
                .ins()
                .store(MemFlagsData::new(), parameters[0], address, 0);
            vec![previous]
        },
    )?;
    define(
        module,
        tables.as_deref_mut(),
        "klffe_metadata",
        &[],
        &[pointer],
        |builder, module, _| {
            let which = load_selected(builder, module, selected, pointer);
            let null = builder.ins().iconst(pointer, 0);
            vec![dispatch(builder, module, which, &metadata, &[], null)]
        },
    )?;
    define(
        module,
        tables.as_deref_mut(),
        "klffe_phase",
        &[types::I64; 4],
        &[types::I64],
        |builder, module, parameters| {
            let which = load_selected(builder, module, selected, pointer);
            let bad = builder.ins().iconst(types::I64, -1);
            vec![dispatch(builder, module, which, &phases, parameters, bad)]
        },
    )?;
    emit_main(module, &inits, tables)
}

/// `main` sets up every definition's globals, gives the runtime the stack
/// top, and hands over to the runtime's driver.
fn emit_main(
    module: &mut ObjectModule,
    inits: &[FuncId],
    tables: Option<&mut UnwindTables>,
) -> Result<(), EmitError> {
    let pointer = module.target_config().pointer_type();
    let stack_init = import(
        module,
        "klfrt_stack_init",
        &signature(module, &[pointer], &[]),
    )?;
    let driver = import(
        module,
        "klffe_main",
        &signature(module, &[types::I32, pointer], &[types::I32]),
    )?;
    define(
        module,
        tables,
        "main",
        &[types::I32, pointer],
        &[types::I32],
        |builder, module, parameters| {
            for init in inits {
                let callee = module.declare_func_in_func(*init, builder.func);
                builder.ins().call(callee, &[]);
            }
            let top = builder.ins().get_stack_pointer(pointer);
            let callee = module.declare_func_in_func(stack_init, builder.func);
            builder.ins().call(callee, &[top]);
            let callee = module.declare_func_in_func(driver, builder.func);
            let call = builder.ins().call(callee, parameters);
            vec![builder.inst_results(call)[0]]
        },
    )
}

fn signature(
    module: &ObjectModule,
    parameters: &[types::Type],
    results: &[types::Type],
) -> Signature {
    let mut signature = module.make_signature();
    signature
        .params
        .extend(parameters.iter().map(|ty| AbiParam::new(*ty)));
    signature
        .returns
        .extend(results.iter().map(|ty| AbiParam::new(*ty)));
    signature
}

fn import(
    module: &mut ObjectModule,
    name: &str,
    signature: &Signature,
) -> Result<FuncId, EmitError> {
    module
        .declare_function(name, Linkage::Import, signature)
        .map_err(|e| err(format!("declare {name}: {e}")))
}

/// Defines the exported function `name`, whose body `body` builds from its
/// parameters and returns the results of.
fn define(
    module: &mut ObjectModule,
    tables: Option<&mut UnwindTables>,
    name: &str,
    parameters: &[types::Type],
    results: &[types::Type],
    body: impl FnOnce(&mut FunctionBuilder, &mut ObjectModule, &[Value]) -> Vec<Value>,
) -> Result<(), EmitError> {
    let signature = signature(module, parameters, results);
    let id = module
        .declare_function(name, Linkage::Export, &signature)
        .map_err(|e| err(format!("declare {name}: {e}")))?;
    let mut context = Context::new();
    context.func.signature = signature;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut fbctx);
        let block = builder.create_block();
        builder.append_block_params_for_function_params(block);
        builder.switch_to_block(block);
        builder.seal_block(block);
        let given = builder.block_params(block).to_vec();
        let returned = body(&mut builder, module, &given);
        builder.ins().return_(&returned);
        let config = module.target_config();
        builder.finalize(config);
    }
    super::entry::define(module, id, &mut context, tables)
        .map_err(|e| err(format!("define {name}: {e}")))
}

fn data_address(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    data: DataId,
    pointer: types::Type,
) -> Value {
    let global = module.declare_data_in_func(data, builder.func);
    builder.ins().symbol_value(pointer, global)
}

fn load_selected(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    selected: DataId,
    pointer: types::Type,
) -> Value {
    let address = data_address(builder, module, selected, pointer);
    builder
        .ins()
        .load(types::I64, MemFlagsData::new(), address, 0)
}

/// Calls `targets[which]` with `arguments` and answers its one result, or
/// `fallback` when `which` names no target.
fn dispatch(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    which: Value,
    targets: &[FuncId],
    arguments: &[Value],
    fallback: Value,
) -> Value {
    let ty = builder.func.dfg.value_type(fallback);
    let done = builder.create_block();
    builder.append_block_param(done, ty);
    for (index, target) in targets.iter().enumerate() {
        let call_block = builder.create_block();
        let next = builder.create_block();
        let matched =
            builder
                .ins()
                .icmp_imm_s(IntCC::Equal, which, i64::try_from(index).unwrap_or(-1));
        builder.ins().brif(matched, call_block, &[], next, &[]);
        builder.seal_block(call_block);
        builder.seal_block(next);
        builder.switch_to_block(call_block);
        let callee: FuncRef = module.declare_func_in_func(*target, builder.func);
        let call = builder.ins().call(callee, arguments);
        let result = builder.inst_results(call)[0];
        builder.ins().jump(done, &[result.into()]);
        builder.switch_to_block(next);
    }
    builder.ins().jump(done, &[fallback.into()]);
    builder.seal_block(done);
    builder.switch_to_block(done);
    builder.block_params(done)[0]
}
