//! `klf-cranelift`: a second native backend (section 16). It lowers verified
//! same `klf-native-rt` runtime the LLVM backend uses. Cranelift is SSA with
//! native block parameters, so KIR blocks map across directly.

mod cache;
mod compile;
mod debug_info;
mod lower;
mod reuse;
mod runtime;
mod unwind;

use cranelift::codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift::codegen::settings::{self, Configurable};
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use klf_kir::{Module as KirModule, Type};
use lower::{FnLowerer, ModuleTables};
use runtime::Runtime;
use std::fmt;
use debug_info::DebugTables;
use unwind::UnwindTables;

pub use cache::CodeCache;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmitError(pub String);

impl fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EmitError {}

fn err(message: impl Into<String>) -> EmitError {
    EmitError(message.into())
}

/// The targets this backend emits objects for.
pub const SUPPORTED_TARGETS: [&str; 6] = [
    "aarch64-apple-darwin",
    "aarch64-apple-ios",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-gnu",
];

/// The object module for `triple`, and the target its functions compile for.
///
/// A Tessera triple compiles as its Linux triple (KLF-RIDL 5.8).
fn new_module(triple: &str) -> Result<(ObjectModule, OwnedTargetIsa), EmitError> {
    let machine = klf_target::machine_triple(triple);
    let triple = machine.as_str();
    if !SUPPORTED_TARGETS.contains(&triple) {
        return Err(err(format!(
            "the Cranelift backend does not support target `{triple}`"
        )));
    }
    let mut flags = settings::builder();
    flags
        .set("is_pic", "true")
        .map_err(|e| err(format!("cranelift flag: {e}")))?;
    flags
        .set("opt_level", "speed")
        .map_err(|e| err(format!("cranelift flag: {e}")))?;
    // KIR is verified before it is lowered (KLF-RIDL 11.7); Cranelift's own
    // check of the IR made from it re-proves what that already holds, and
    // was the most expensive single pass of a compile.
    flags
        .set("enable_verifier", "false")
        .map_err(|e| err(format!("cranelift flag: {e}")))?;
    // Every frame links to its caller's, a leaf's too, so a debugger and a
    // sampling profiler walk the whole Kira call chain (section 16.3).
    flags
        .set("preserve_frame_pointers", "true")
        .map_err(|e| err(format!("cranelift flag: {e}")))?;
    // Thread-local globals are the platform's own thread-local data.
    let tls_model = if triple.contains("apple") {
        Some("macho")
    } else if triple.contains("windows") {
        Some("coff")
    } else if triple.contains("linux") {
        Some("elf_gd")
    } else {
        None
    };
    if let Some(model) = tls_model {
        flags
            .set("tls_model", model)
            .map_err(|e| err(format!("cranelift flag: {e}")))?;
    }
    // Windows commits a thread's stack one guard page at a time, so a frame
    // larger than a page touches each of its pages on the way down.
    if triple.contains("windows") {
        for (flag, value) in [
            ("enable_probestack", "true"),
            ("probestack_strategy", "inline"),
        ] {
            flags
                .set(flag, value)
                .map_err(|e| err(format!("cranelift flag: {e}")))?;
        }
    }
    let parsed: target_lexicon::Triple = triple
        .parse()
        .map_err(|e| err(format!("target `{triple}`: {e}")))?;
    let isa = cranelift::codegen::isa::lookup(parsed)
        .map_err(|e| err(format!("target `{triple}`: {e}")))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| err(format!("isa finish: {e}")))?;
    let builder = ObjectBuilder::new(
        isa.clone(),
        "klf",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| err(format!("object builder: {e}")))?;
    Ok((ObjectModule::new(builder), isa))
}

/// The object symbol of a function the module does not export.
#[must_use]
pub fn local_symbol(name: &str) -> String {
    format!("kir.{name}")
}

/// The declarations of a [`build`]: each function's id, whether it is
/// exported under a symbol of its own, and the symbol and linkage it was
/// declared with, by position.
type Declarations<'a> = (
    lower::IdMap<'a>,
    Vec<bool>,
    Vec<Option<(String, Linkage)>>,
);

/// Declares every function of `module` that has a body or that something
/// names, with the linkage `symbol_for` gives it.
fn declare_functions<'a>(
    object: &mut ObjectModule,
    module: &'a KirModule,
    symbol_for: &dyn Fn(&str) -> Option<String>,
) -> Result<Declarations<'a>, EmitError> {
    let mut ids = lower::IdMap::default();
    // A foreign function nothing names is left undeclared: an import is an
    // undefined symbol whether or not anything calls it, and a call into a
    // library the target leaves out traps rather than naming its function.
    let named = module.named_functions();
    let mut exported = vec![false; module.functions.len()];
    let mut declarations: Vec<Option<(String, Linkage)>> = vec![None; module.functions.len()];
    // Two definitions under one symbol would only fail later, as a
    // duplicate definition naming neither; imports may share one.
    let mut symbols: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for (position, function) in module.functions.iter().enumerate() {
        if function.foreign && !named.contains(&function.name) {
            continue;
        }
        let signature = lower::signature(object, function)?;
        let (symbol, linkage) = if function.foreign {
            let symbol = if function.c_prototype.is_some() {
                klf_kir::c_shim_symbol(function.name.as_str())
            } else {
                klf_kir::c_symbol(function.name.as_str()).to_owned()
            };
            (symbol, Linkage::Import)
        } else if let Some(symbol) = symbol_for(function.name.as_str()) {
            exported[position] = true;
            (symbol, Linkage::Export)
        } else if function.linkage == klf_kir::Linkage::Exported {
            (function.name.as_str().to_owned(), Linkage::Export)
        } else if function
            .c_prototype
            .as_ref()
            .is_some_and(klf_kir::CPrototype::needs_shim)
        {
            // C enters it through a shim in another object, which must see
            // the symbol; nothing outside the program does.
            (local_symbol(function.name.as_str()), Linkage::Hidden)
        } else {
            (local_symbol(function.name.as_str()), Linkage::Local)
        };
        if linkage != Linkage::Import
            && let Some(earlier) = symbols.insert(symbol.clone(), function.name.as_str())
        {
            return Err(err(format!(
                "`{}` and `{earlier}` are both given the symbol `{symbol}`",
                function.name
            )));
        }
        let id = object
            .declare_function(&symbol, linkage, &signature)
            .map_err(|e| err(format!("declare `{}`: {e}", function.name)))?;
        ids.insert(function.name.as_str(), (id, signature));
        declarations[position] = Some((symbol, linkage));
    }
    Ok((ids, exported, declarations))
}

/// Declares every function of `module` and lowers each non-foreign body.
///
/// `symbol_for` decides the linkage of each function; it returns `None` for
/// the default of a local `kir.<name>`.
fn build<'a>(
    object: &mut ObjectModule,
    isa: &dyn TargetIsa,
    cache: Option<&mut CodeCache>,
    module: &'a KirModule,
    declared: &Declared<'a>,
    symbol_for: &dyn Fn(&str) -> Option<String>,
) -> Result<(lower::IdMap<'a>, Option<UnwindTables>, DebugTables), EmitError> {
    let Declared {
        runtime,
        globals,
        strings,
        pending,
    } = declared;
    // Every function gets call frame information where the platform reads
    // it, so a debugger and a crash report walk the whole stack, through a
    // frameless leaf too.
    let unwinding = unwind::supported(isa);
    let (ids, exported, declarations) = declare_functions(object, module, symbol_for)?;

    let index = klf_kir::ModuleIndex::of(module);
    let shims = declare_shims(object, module, &index, &ids)?;
    let recursive = module.recursive_functions();
    let earlier = cache.as_deref().map(CodeCache::entries);
    // Code is only taken again from a cache of earlier builds.
    let items = earlier.map(|_| reuse::Items::of(module, &declarations, isa, pending.is_some()));
    let shared: &ObjectModule = object;
    let bodies: Vec<usize> = (0..module.functions.len())
        .filter(|&position| !module.functions[position].foreign)
        .collect();
    let compiled = compile::in_order(
        bodies.len(),
        || {
            (
                cranelift::frontend::FunctionBuilderContext::new(),
                cranelift::codegen::Context::new(),
                klf_layout::Layouts::new(&module.types),
            )
        },
        |(builder, context, layouts), body| {
            let position = bodies[body];
            let function = &module.functions[position];
            let (id, signature) = ids
                .get(function.name.as_str())
                .cloned()
                .ok_or_else(|| err("missing function id"))?;
            // A function exported under a symbol of its own can be called
            // from outside, and so back into a cycle.
            let counts_depth = recursive.contains(&position) || exported[position];
            let key = items.as_ref().and_then(|items| items.key(function, counts_depth));
            if let Some((key, code)) = key.and_then(|key| Some((key, earlier?.get(&key)?)))
                && let Some(recalled) = reuse::recalled(shared, id, key, code)
            {
                return Ok((function.name.as_str().to_owned(), recalled));
            }
            let mut clif = cranelift::codegen::ir::Function::with_name_signature(
                cranelift::codegen::ir::UserFuncName::default(),
                signature,
            );
            let tables = ModuleTables {
                runtime,
                ids: &ids,
                shims: &shims,
                globals,
                strings,
                index: &index,
                layouts,
                unwind: *pending,
            };
            let mut lowerer = FnLowerer::new(
                shared,
                &mut clif,
                builder,
                &tables,
                counts_depth,
            );
            if let Err(error) = lowerer.lower(function) {
                drop(lowerer);
                // A function given up halfway leaves the worker's builder
                // context in the middle of it, and the next function would
                // start there.
                *builder = cranelift::frontend::FunctionBuilderContext::new();
                return Err(error);
            }
            let places = std::mem::take(&mut lowerer.places);
            lowerer.builder.finalize(shared.target_config());
            let mut compiled = compile::compile(isa, earlier, unwinding, context, id, clif, &places)
                .map_err(|e| err(format!("compile `{}`: {e}", function.name)))?;
            if let Some(key) = key
                && let Some(code) = reuse::remembered(shared, &compiled)
            {
                compiled.fresh.push((key, code));
            }
            Ok::<_, EmitError>((function.name.as_str().to_owned(), compiled))
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let mut unwind = unwinding.then(UnwindTables::default);
    let mut used = compile::CacheUse::default();
    let mut debug = DebugTables::of(module);
    compile::define_all(object, compiled, &mut unwind, &mut debug, &mut used)?;
    if let Some(cache) = cache {
        cache.settle(&used.reused, used.fresh);
    }
    Ok((ids, unwind, debug))
}

/// The C entry of each function a `func.ref` takes the address of whose C
/// prototype needs a shim: C calls it through the shim, in another object.
fn declare_shims<'a>(
    object: &mut ObjectModule,
    module: &'a KirModule,
    index: &klf_kir::ModuleIndex<'a>,
    ids: &lower::IdMap<'a>,
) -> Result<lower::ShimMap<'a>, EmitError> {
    let mut shims = lower::ShimMap::default();
    // Most programs hand C no function that needs one.
    let shimmed = |function: &klf_kir::Function| {
        !function.foreign
            && function
                .c_prototype
                .as_ref()
                .is_some_and(klf_kir::CPrototype::needs_shim)
    };
    if !module.functions.iter().any(shimmed) {
        return Ok(shims);
    }
    let referenced = module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match &instruction.operation {
            klf_kir::Operation::FunctionReference(function) => Some(function.as_str()),
            _ => None,
        });
    for function in referenced {
        let needs = index.functions.get(function).is_some_and(|declared| shimmed(declared));
        if !needs || shims.contains_key(function) {
            continue;
        }
        let Some((_, signature)) = ids.get(function) else {
            continue;
        };
        let id = object
            .declare_function(&klf_kir::c_shim_symbol(function), Linkage::Import, signature)
            .map_err(|e| err(format!("declare the C entry of `@{function}`: {e}")))?;
        shims.insert(function, id);
    }
    Ok(shims)
}

/// The runtime functions, globals and strings every lowered function refers
/// to, declared in the object before any function is.
struct Declared<'a> {
    runtime: Runtime,
    globals: lower::GlobalMap<'a>,
    strings: lower::StringMap,
    /// The pending count of cancellation polls, when the object's functions
    /// get pads and polls.
    pending: Option<cranelift_module::DataId>,
}

impl<'a> Declared<'a> {
    fn new(
        object: &mut ObjectModule,
        module: &'a KirModule,
        unwinding: bool,
    ) -> Result<Self, EmitError> {
        Ok(Self {
            runtime: Runtime::declare(object, unwinding)?,
            globals: lower::declare_globals(object, module)?,
            strings: lower::declare_strings(object, module)?,
            pending: if unwinding {
                Some(lower::declare_pending(object)?)
            } else {
                None
            },
        })
    }
}

/// The object's bytes, with `tables` written into it first.
fn finish_with(
    object: ObjectModule,
    tables: Option<UnwindTables>,
    debug: DebugTables,
    unit_name: &str,
    isa: &dyn TargetIsa,
) -> Result<Vec<u8>, EmitError> {
    let mut product = object.finish();
    if let Some(tables) = tables {
        tables.write(&mut product, isa)?;
    }
    debug.write(&mut product, unit_name)?;
    product.emit().map_err(|e| err(format!("object emit: {e}")))
}

/// Lower `module`, add a `main` that calls `entry` with `arguments`, and emit a
/// relocatable object. The object imports the `klfrt_*` runtime and reports the
/// outcome through `klfrt_finish` exactly as the LLVM backend does.
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower.
pub fn emit_object(
    module: &KirModule,
    entry: &str,
    arguments: &[u64],
) -> Result<Vec<u8>, EmitError> {
    emit_object_with(module, entry, arguments, EntryConvention::Outcome)
}

/// How the synthesized `main` ends a program.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EntryConvention {
    /// Report the outcome as a `RET`/`TRAP` line and exit 0, the protocol the
    /// kernel's test drivers read.
    #[default]
    Outcome,
    /// A C process: `main(argc, argv)` passes its arguments to the entry, the
    /// entry's value is the exit status, and a trap is reported on stderr and
    /// aborts.
    Process,
    /// Process convention where a trap exits with status 1.
    ProcessExit,
}

/// [`emit_object`] with the entry convention of the language being built.
/// Under [`EntryConvention::Process`] `arguments` is unused.
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower.
pub fn emit_object_with(
    module: &KirModule,
    entry: &str,
    arguments: &[u64],
    convention: EntryConvention,
) -> Result<Vec<u8>, EmitError> {
    program_object(module, entry, arguments, convention, &[], None)
}

/// [`emit_object_with`], taking each function's machine code from `cache`
/// when an earlier build compiled the same function, and leaving in `cache`
/// the code of this build for the next.
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower.
pub fn emit_object_cached(
    module: &KirModule,
    entry: &str,
    arguments: &[u64],
    convention: EntryConvention,
    cache: &mut CodeCache,
) -> Result<Vec<u8>, EmitError> {
    program_object(module, entry, arguments, convention, &[], Some(cache))
}

/// [`emit_object_with`], also exporting each `(kir_name, symbol)` pair in
/// `exports` under `symbol`, as a C program's functions are visible to the
/// modules it loads. `cache`, when given, is used as by
/// [`emit_object_cached`].
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower, or when an
/// export names a function the module does not define.
pub fn emit_object_exporting(
    module: &KirModule,
    entry: &str,
    convention: EntryConvention,
    exports: &[(&str, &str)],
    cache: Option<&mut CodeCache>,
) -> Result<Vec<u8>, EmitError> {
    program_object(module, entry, &[], convention, exports, cache)
}

fn program_object(
    module: &KirModule,
    entry: &str,
    arguments: &[u64],
    convention: EntryConvention,
    exports: &[(&str, &str)],
    cache: Option<&mut CodeCache>,
) -> Result<Vec<u8>, EmitError> {
    let legal = klf_kir::legalize(module);
    let module = legal.as_ref().unwrap_or(module);
    let export_map: std::collections::HashMap<&str, &str> = exports.iter().copied().collect();
    for (name, _) in exports {
        if !module.functions.iter().any(|f| f.name.as_str() == *name && !f.foreign) {
            return Err(err(format!("export names unknown function `{name}`")));
        }
    }
    let (mut object, isa) = new_module(module.target.triple())?;
    let declared = Declared::new(&mut object, module, false)?;
    let (ids, tables, debug) = build(&mut object, &*isa, cache, module, &declared, &|name| {
        export_map.get(name).map(|symbol| (*symbol).to_owned())
    })?;
    let program = lower::ProgramEntry {
        name: entry,
        arguments,
        convention,
    };
    lower::emit_main(
        &mut object,
        module,
        &program,
        &declared.runtime,
        &ids,
        &declared.globals,
    )?;
    finish_with(object, tables, debug, module.name.as_str(), &*isa)
}

/// Lower `module` as a library: each `(kir_name, symbol)` pair in `exports` is
/// emitted as a public symbol under `symbol` with the platform C convention
/// (the register-level realization of `ridlcc` for trivial scalar functions,
/// section 22.7); every other function stays a local `kir.<name>`. No `main` is
/// added, so the object links into another program.
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower, or when an
/// export names a function the module does not define.
pub fn emit_library_object(
    module: &KirModule,
    exports: &[(&str, &str)],
) -> Result<Vec<u8>, EmitError> {
    emit_library_object_with_init(module, exports, None)
}

/// [`emit_library_object`], plus an exported `init` function that allocates
/// and initializes the module's globals, as a program's `main` does. The
/// caller arranges for `init` to run before any export is called.
///
/// # Errors
/// Returns [`EmitError`] as [`emit_library_object`] does, or when the module
/// defines globals and no `init` symbol is given.
pub fn emit_library_object_with_init(
    module: &KirModule,
    exports: &[(&str, &str)],
    init: Option<&str>,
) -> Result<Vec<u8>, EmitError> {
    library_object(module, exports, init, None)
}

/// [`emit_library_object_with_init`], taking each function's machine code
/// from `cache` when an earlier build compiled the same function, and
/// leaving in `cache` the code this build used.
///
/// # Errors
/// Returns [`EmitError`] as [`emit_library_object_with_init`] does.
pub fn emit_library_object_cached(
    module: &KirModule,
    exports: &[(&str, &str)],
    init: Option<&str>,
    cache: &mut CodeCache,
) -> Result<Vec<u8>, EmitError> {
    library_object(module, exports, init, Some(cache))
}

fn library_object(
    module: &KirModule,
    exports: &[(&str, &str)],
    init: Option<&str>,
    cache: Option<&mut CodeCache>,
) -> Result<Vec<u8>, EmitError> {
    let legal = klf_kir::legalize(module);
    let module = legal.as_ref().unwrap_or(module);
    let export_map: std::collections::HashMap<&str, &str> = exports.iter().copied().collect();
    for (name, _) in exports {
        if !module.functions.iter().any(|f| f.name.as_str() == *name) {
            return Err(err(format!("export names unknown function `{name}`")));
        }
    }

    let (mut object, isa) = new_module(module.target.triple())?;
    let declared = Declared::new(&mut object, module, false)?;
    let (ids, tables, debug) = build(&mut object, &*isa, cache, module, &declared, &|name| {
        export_map.get(name).map(|symbol| (*symbol).to_owned())
    })?;
    if let Some(init) = init {
        lower::emit_library_init(
            &mut object,
            module,
            init,
            &declared.runtime,
            &ids,
            &declared.globals,
        )?;
    }
    finish_with(object, tables, debug, module.name.as_str(), &*isa)
}

/// Lower a definition package's phase `module` into one object of a native
/// frontend binary (`KLF-KSL.md` section 5.9), as definition `index` of the
/// binary. On top of the definition's own code, which stays local to the
/// object, it exports `klffe_metadata_N`, a `klffe_phase_N` trampoline over
/// the `Language` phases, and `klffe_init_N`, the global prologue.
///
/// `metadata` is the descriptor blob the runtime reads back: see
/// `klf_native_rt::frontend` for its shape.
///
/// On ELF targets every function gets unwind tables, the definition's own
/// functions get cleanup pads and cancellation polls, so a language server
/// can stop and unwind them (`KLF-EDITOR.md` section 8.6).
///
/// # Errors
/// Returns [`EmitError`] for anything the backend cannot lower, or when the
/// module is missing one of the four phases.
pub fn emit_frontend_object(
    module: &KirModule,
    metadata: &str,
    index: usize,
) -> Result<Vec<u8>, EmitError> {
    let (mut object, isa) = new_module(module.target.triple())?;
    let declared = Declared::new(&mut object, module, unwind::supported(&*isa))?;
    let (ids, mut tables, debug) = build(&mut object, &*isa, None, module, &declared, &|_| None)?;
    lower::emit_frontend_entry(
        &mut object,
        module,
        metadata,
        index,
        &declared.runtime,
        &ids,
        &declared.globals,
        tables.as_mut(),
    )?;
    finish_with(object, tables, debug, module.name.as_str(), &*isa)
}

/// The registry object that joins `count` frontend objects into one binary:
/// `main`, `klffe_select`, `klffe_language_count`, `klffe_language_metadata`,
/// and `klffe_metadata` and `klffe_phase` for the selected definition.
///
/// # Errors
/// Returns [`EmitError`] when `triple` has no backend.
pub fn emit_frontend_registry(triple: &str, count: usize) -> Result<Vec<u8>, EmitError> {
    let (mut object, isa) = new_module(triple)?;
    let mut tables = unwind::supported(&*isa).then(UnwindTables::default);
    lower::emit_registry(&mut object, count, tables.as_mut())?;
    finish_with(object, tables, DebugTables::default(), "klf registry", &*isa)
}

fn is_void(ty: &Type) -> bool {
    matches!(ty, Type::Unit | Type::Never)
}
