//! Native outputs of a generated frontend: linked executables, static and
//! shared libraries, and the sysroot they link against (section 5.4).
//!
//! A driver installed in a sysroot at `<out>/bin/<driver>` links executables
//! against the kernel runtime (`runtime`) and, when the language's std was
//! built, `<out>/lib/<lang>/<target>/libstd.a`.

use crate::runtime::Runtime;
use crate::toolchain::{CToolchain, check_target, find_tool, is_apple, is_windows, unwinder};
use klf_defsvc::FrontendMetadata;
use klf_kir::Module;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The stack a Windows executable reserves for its main thread, in bytes:
/// the 8 MiB Linux and macOS give a main thread is a quarter of it, and
/// Windows reserves only 1 MiB unless the executable asks.
const WINDOWS_STACK_RESERVE: u64 = 32 * 1024 * 1024;

pub(crate) fn emit_binary(
    module: Module,
    output: &Path,
    language: &str,
    entry: klf_services::EntryConvention,
    level: klf_kir_passes::Level,
) -> Result<(), String> {
    let target = module.target.triple().to_owned();
    check_target(&target)?;
    let toolchain = CToolchain::for_target(&target)?;
    let module = machine_module(module, level, output)?;
    klf_diag::progress!("codegen");
    let convention = match entry {
        klf_services::EntryConvention::Outcome => klf_cranelift::EntryConvention::Outcome,
        klf_services::EntryConvention::Process => klf_cranelift::EntryConvention::Process,
        klf_services::EntryConvention::ProcessExit => klf_cranelift::EntryConvention::ProcessExit,
    };
    // What the program links besides its own machine code depends only on
    // the module, so it is built while Cranelift compiles.
    let scratch = Scratch::new()?;
    let exports = crate::module_pruning::program_exports(&module);
    let exports: Vec<(&str, &str)> = exports.iter().map(|name| (*name, *name)).collect();
    std::thread::scope(|scope| {
        let natives = scope.spawn(|| Natives::of(&module, language, &target, &toolchain, &scratch));
        let (object, cache) =
            match klf_cranelift::CodeCache::for_artifact(module.name.as_str(), output, &target) {
                Some(mut cache) => {
                    klf_diag::progress!("compile");
                    let object = klf_cranelift::emit_object_exporting(
                        &module,
                        "main",
                        convention,
                        &exports,
                        Some(&mut cache),
                    )
                    .map_err(|error| format!("Cranelift emission failed: {error:?}"))?;
                    (object, Some(cache))
                }
                None => (
                    klf_cranelift::emit_object_exporting(&module, "main", convention, &exports, None)
                        .map_err(|error| format!("Cranelift emission failed: {error:?}"))?,
                    None,
                ),
            };
        // The machine code is kept while the program links. The cache only
        // saves work; a build that cannot keep it still stands.
        if let Some(cache) = cache {
            scope.spawn(move || {
                if let Err(error) = cache.save() {
                    klf_diag::err!(
                        "warning: the machine code of this build could not be cached: {error}"
                    );
                }
            });
        }
        let object = scratch.write("program.o", &object)?;
        klf_diag::progress!("native objects");
        let natives = natives
            .join()
            .map_err(|_| "building the native objects panicked".to_owned())??;
        klf_diag::progress!("link");
        link_binary(&module, &object, &natives, output, &target, &toolchain)
    })
}

/// The archives a binary built from `module` links besides its own machine
/// code, the runtime and the std library, when they are all it links:
/// `None` when the module names C sources, libraries, link flags or
/// resources of its own.
pub(crate) fn linked_archives(
    module: &Module,
    language: &str,
    target: &str,
) -> Option<Vec<PathBuf>> {
    if module
        .attributes
        .iter()
        .any(|attribute| attribute.name.starts_with("link."))
    {
        return None;
    }
    let mut archives = vec![Runtime::of(module).archive(target).ok()?];
    if let Some(root) = installed_sysroot() {
        archives.extend(carried_std(&root, language, target).ok()?);
    }
    Some(archives)
}

/// What a binary links besides its own machine code: the runtime, the std
/// library and the C objects the module names.
struct Natives {
    runtime: Runtime,
    runtime_archive: PathBuf,
    std_library: Option<PathBuf>,
    shims: Option<PathBuf>,
    sources: Vec<PathBuf>,
    imports: Vec<PathBuf>,
}

impl Natives {
    fn of(
        module: &Module,
        language: &str,
        target: &str,
        toolchain: &CToolchain,
        scratch: &Scratch,
    ) -> Result<Self, String> {
        let runtime = Runtime::of(module);
        let runtime_archive = runtime.archive(target)?;
        let std_library = match installed_sysroot() {
            Some(root) => carried_std(&root, language, target)?,
            None => None,
        };
        let shims = crate::native_link::c_shim_object(
            module,
            toolchain,
            target,
            scratch,
            &klf_cranelift::local_symbol,
        )?;
        let sources = crate::native_link::c_source_objects(module, toolchain, target, scratch)?;
        // A binary keeps the names of the functions it exports, which the
        // adapters' buffer functions are.
        let imports = ridl_import_objects(module, toolchain, target, scratch, &|name: &str| {
            name.to_owned()
        })?;
        Ok(Self {
            runtime,
            runtime_archive,
            std_library,
            shims,
            sources,
            imports,
        })
    }
}

/// Links `object`, the machine code of `module`, with `natives` into the
/// binary at `output`.
fn link_binary(
    module: &Module,
    object: &Path,
    natives: &Natives,
    output: &Path,
    target: &str,
    toolchain: &CToolchain,
) -> Result<(), String> {
    let mut command = toolchain.command();
    // lld links the program and the std archive several times faster than
    // the system's bfd linker.
    command.arg("-fuse-ld=lld");
    command.arg(if is_apple(target) {
        "-Wl,-dead_strip"
    } else {
        "-Wl,--gc-sections"
    });
    if is_windows(target) {
        command.arg(format!("-Wl,--stack,{WINDOWS_STACK_RESERVE}"));
    }
    // A program exporting its functions is one that loads modules: they also
    // bind to the runtime and the std it links, which stripping would drop.
    if !is_windows(target)
        && module
            .attributes
            .iter()
            .any(|attribute| attribute.name == "program.export")
    {
        command.arg(if is_apple(target) {
            "-Wl,-export_dynamic"
        } else {
            "-Wl,--export-dynamic"
        });
    }
    let result = command
        .arg(object)
        .args(natives.shims.iter())
        .args(natives.sources.iter())
        .args(natives.imports.iter())
        .args(crate::native_link::link_arguments(module, target))
        .args(natives.std_library.iter())
        .args(natives.runtime.link_arguments(target))
        .arg(&natives.runtime_archive)
        .args(system_libraries(target))
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|error| format!("linking `main` failed: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "linking `main` failed\n{}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    if is_apple(target) {
        crate::native_link::gather_debug_information(output);
    }
    crate::native_link::stage_resources(module)
}

/// Writes `module` as a static library. Every defined function that is not
/// private is exported under its own name with the platform C convention, so
/// a program declares it as a foreign function to call it. A module with
/// globals also gets an initializer that runs when the program loads.
pub(crate) fn emit_library(
    module: &Module,
    output: &Path,
    language: &str,
    export: Option<&str>,
) -> Result<(), String> {
    let (_scratch, object) = library_object(module, output, language, export)?;
    if output.exists() {
        std::fs::remove_file(output)
            .map_err(|error| format!("cannot replace {}: {error}", output.display()))?;
    }
    let archiver = find_tool("llvm-ar").unwrap_or_else(|| PathBuf::from("ar"));
    let result = Command::new(archiver)
        .arg("rcs")
        .arg(output)
        .arg(&object)
        .output()
        .map_err(|error| format!("archiver failed: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "archiving {} failed\n{}",
            output.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}

/// Writes `module` as a shared library with the same exports as
/// [`emit_library`], linked against the native libraries it declares.
/// Runtime symbols stay undefined and bind to the runtime
/// the loading program links, except on Windows, where a DLL resolves every
/// symbol when it is linked and so carries its own runtime.
pub(crate) fn emit_shared(module: &Module, output: &Path, language: &str) -> Result<(), String> {
    let target = module.target.triple().to_owned();
    let (_scratch, object) = library_object(module, output, language, None)?;
    let toolchain = CToolchain::for_target(&target)?;
    let mut command = toolchain.command();
    if is_apple(&target) {
        command.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
    } else if is_windows(&target) {
        command
            .args(["-shared", "-fuse-ld=lld"])
            .arg(Runtime::Native.archive(&target)?)
            .args(system_libraries(&target));
    } else {
        command.args(["-shared", "-nostdlib", "-fuse-ld=lld"]);
    }
    // The native libraries the module declares are the shared library's own
    // dependencies, recorded as it is linked so a loader finds them.
    let result = command
        .arg(&object)
        .args(crate::native_link::link_arguments(module, &target))
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|error| format!("linking {} failed: {error}", output.display()))?;
    if !result.status.success() {
        return Err(format!(
            "linking {} failed\n{}",
            output.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}

/// The library's relocatable object: the lowered module, merged with a
/// load-time constructor when the module has globals to initialize. The
/// object lives in the returned scratch directory.
fn library_object(
    module: &Module,
    output: &Path,
    language: &str,
    export: Option<&str>,
) -> Result<(Scratch, PathBuf), String> {
    let target = module.target.triple().to_owned();
    check_target(&target)?;
    let mut module = instantiated(module.clone())?;
    let surface = crate::ridl_surface::described(&module)?;
    let exports = crate::ridl_surface::add_ridl_symbols(
        &mut module,
        language,
        export,
        surface.as_ref().map(|surface| surface.module.name.as_str()),
    );
    let exports: Vec<(&str, &str)> = exports
        .iter()
        .map(|(name, symbol)| (name.as_str(), symbol.as_str()))
        .collect();
    let has_initializers = module.functions.iter().any(|function| {
        function
            .attributes
            .contains(&klf_kir::FunctionAttribute::Initializer)
    });
    let init = (!module.globals.is_empty() || has_initializers).then(|| init_symbol(output));
    let object = match klf_cranelift::CodeCache::for_artifact(module.name.as_str(), output, &target)
    {
        Some(mut cache) => {
            let object = klf_cranelift::emit_library_object_cached(
                &module,
                &exports,
                init.as_deref(),
                &mut cache,
            )
            .map_err(|error| format!("Cranelift emission failed: {error:?}"))?;
            // The cache only saves work; a build that cannot keep it still
            // stands.
            if let Err(error) = cache.save() {
                klf_diag::err!(
                    "warning: the machine code of this build could not be cached: {error}"
                );
            }
            object
        }
        None => klf_cranelift::emit_library_object_with_init(&module, &exports, init.as_deref())
            .map_err(|error| format!("Cranelift emission failed: {error:?}"))?,
    };
    let scratch = Scratch::new()?;
    let object_path = scratch.write("library.o", &object)?;
    let toolchain = CToolchain::for_target(&target)?;
    let defined = |name: &str| {
        exports
            .iter()
            .find(|(exported, _)| *exported == name)
            .map_or_else(
                || klf_cranelift::local_symbol(name),
                |(_, symbol)| (*symbol).to_owned(),
            )
    };
    let shims =
        crate::native_link::c_shim_object(&module, &toolchain, &target, &scratch, &defined)?;
    let sources = crate::native_link::c_source_objects(&module, &toolchain, &target, &scratch)?;
    let entries = match &surface {
        Some(surface) => Some(ridl_entry_object(
            surface, &toolchain, &target, &scratch, &defined,
        )?),
        None => None,
    };
    let imports = ridl_import_objects(&module, &toolchain, &target, &scratch, &defined)?;
    if init.is_none()
        && shims.is_none()
        && sources.is_empty()
        && entries.is_none()
        && imports.is_empty()
    {
        return Ok((scratch, object_path));
    }
    let directory = scratch.0.clone();
    let mut parts = vec![object_path.clone()];
    parts.extend(shims);
    parts.extend(sources);
    parts.extend(entries);
    parts.extend(imports);
    if let Some(init) = init {
        let constructor = format!(
            "extern void {init}(void);\n__attribute__((constructor)) static void klf_library_constructor(void) {{ {init}(); }}\n"
        );
        parts.push(crate::native_link::generated_object(
            "c",
            &constructor,
            &[],
            &toolchain,
            &target,
            &scratch,
        )?);
    }
    let merged = directory.join("merged.o");
    let mut arguments: Vec<&std::ffi::OsStr> = vec!["-r".as_ref(), "-nostdlib".as_ref()];
    if !is_apple(&target) {
        arguments.push("-fuse-ld=lld".as_ref());
    }
    arguments.extend(parts.iter().map(|part| part.as_os_str()));
    arguments.extend(["-o".as_ref(), merged.as_os_str()]);
    toolchain.run(&arguments)?;
    Ok((scratch, merged))
}

/// The object holding the `ridlcc` entries, descriptors and metadata of a
/// described surface, compiled from LLVM IR by the bundled clang.
fn ridl_entry_object(
    surface: &crate::ridl_surface::Surface,
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
    defined: &dyn Fn(&str) -> String,
) -> Result<PathBuf, String> {
    let source = crate::ridl_entries::source(surface, target, defined)?;
    ir_object(&source, toolchain, target, scratch)
}

/// One object per module the program imports from through RIDL, holding the
/// adapters to that module's `ridlcc` entries, and one per module whose
/// traits the program makes or calls trait objects of.
fn ridl_import_objects(
    module: &Module,
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
    defined: &dyn Fn(&str) -> String,
) -> Result<Vec<PathBuf>, String> {
    let mut objects = Vec::new();
    for imported in crate::ridl_imports::imported(module)?.iter() {
        let source = crate::ridl_imports::source(imported, target, defined)?;
        objects.push(ir_object(&source, toolchain, target, scratch)?);
    }
    let destroys = crate::ridl_surface::destroys(module);
    let own = crate::ridl_surface::module_name(module);
    for (name, witnessed) in crate::ridl_witnesses::witnessed(module).iter() {
        let source =
            crate::ridl_witnesses::source(name, witnessed, &destroys, own, target, defined)?;
        objects.push(ir_object(&source, toolchain, target, scratch)?);
    }
    Ok(objects)
}

/// The object of generated LLVM IR, which names its own target triple.
fn ir_object(
    source: &str,
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
) -> Result<PathBuf, String> {
    crate::native_link::generated_object(
        "ll",
        source,
        &["-Wno-override-module"],
        toolchain,
        target,
        scratch,
    )
}

/// `__klf_init_<stem>`: one initializer symbol per library file name.
fn init_symbol(output: &Path) -> String {
    let stem: String = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("__klf_init_{stem}")
}

/// The system libraries a program links after the runtime: what the runtime's
/// Rust standard library imports (`rustc --print native-static-libs`), with the
/// unwinder the target's own C library carries. Darwin's `libSystem` holds all
/// of it.
fn system_libraries(target: &str) -> Vec<&'static str> {
    let mut libraries: Vec<&'static str> = if is_apple(target) {
        Vec::new()
    } else if is_windows(target) {
        vec![
            "-lkernel32",
            "-lntdll",
            "-luserenv",
            "-lws2_32",
            "-ldbghelp",
        ]
    } else {
        vec!["-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"]
    };
    libraries.push(unwinder(target));
    libraries.retain(|library| !library.is_empty());
    libraries
}

/// `module`, verified as the driver lowered it, with its generic functions
/// instantiated. Only the instances are new, so the result is verified again
/// when there were templates to instantiate.
fn instantiated(module: Module) -> Result<Module, String> {
    if module
        .functions
        .iter()
        .all(|function| function.generics.is_empty())
    {
        return Ok(module);
    }
    let module = klf_kir_passes::monomorphize_owned(module);
    verify(&module)?;
    Ok(module)
}

/// The module an executable written to `output` compiles to machine code:
/// pruned to what its entry reaches, instantiated, and optimized at `level`.
/// `--emit kir` writes this module when the build optimizes, so the KIR read
/// is the KIR Cranelift is given.
pub(crate) fn machine_module(
    module: Module,
    level: klf_kir_passes::Level,
    output: &Path,
) -> Result<Module, String> {
    let target = module.target.triple().to_owned();
    klf_diag::progress!("prune");
    let module = crate::module_pruning::prune_unreachable(module);
    klf_diag::progress!("instantiate");
    let module = instantiated(module)?;
    klf_diag::progress!("optimize");
    optimized(module, level, output, &target)
}

/// Runs section 14.1's tiers for Cranelift, then drops the functions every
/// call to which was inlined. The module optimized at `level` for the artifact written to `output`,
/// with what the last build of that artifact optimized alike taken from the
/// optimizer's cache.
fn optimized(
    mut module: Module,
    level: klf_kir_passes::Level,
    output: &Path,
    target: &str,
) -> Result<Module, String> {
    if level == klf_kir_passes::Level::O0 {
        return Ok(module);
    }
    let cache =
        crate::optimizer_cache::OptimizerCache::for_artifact(module.name.as_str(), output, target);
    klf_kir_passes::optimize_with(
        &mut module,
        level,
        klf_kir_passes::Backend::Cranelift,
        cache.as_ref().map(|cache| cache as &dyn klf_kir_passes::Memo),
    );
    if let Some(cache) = cache {
        cache.settle();
    }
    let module = crate::module_pruning::prune_unreachable(module);
    if cfg!(debug_assertions) {
        verify(&module)?;
    }
    Ok(module)
}

fn verify(module: &Module) -> Result<(), String> {
    let diagnostics = klf_kir_verify::verify_module(module);
    if diagnostics.is_empty() {
        return Ok(());
    }
    let diagnostics = diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("KIR verification failed:\n{diagnostics}"))
}

/// The sysroot this driver is installed in: the parent of its `bin/`, when
/// that directory holds a `definition.lock`.
pub(crate) fn installed_sysroot() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let root = executable.parent()?.parent()?.to_path_buf();
    root.join("definition.lock").exists().then_some(root)
}

/// The std's source directory: `share/<lang>/std` in the sysroot this driver
/// is installed in, otherwise the `stdRoot` the frontend was built from.
pub(crate) fn std_root(metadata: &FrontendMetadata) -> String {
    let installed = installed_sysroot()
        .map(|root| root.join("share").join(&metadata.name).join("std"))
        .filter(|directory| directory.is_dir());
    installed.map_or_else(
        || metadata.std_root.clone(),
        |directory| directory.to_string_lossy().into_owned(),
    )
}

/// `include/<lang>` under a sysroot: the std's headers.
pub(crate) fn std_include(root: &Path, language: &str) -> PathBuf {
    root.join("include").join(language)
}

/// Links the installed std's bytecode into a program, so a `.klbc` with a
/// `main` runs on the VM with its std inside it, the way `--emit bin` links
/// `libstd.a`. A RIDL module runs in an instance of its own, so it carries
/// its std too. Any other library, or a driver outside a sysroot, is left as
/// it is.
pub(crate) fn link_std_bytecode(
    program: &mut klf_vm::Program,
    module: &Module,
    language: &str,
) -> Result<(), String> {
    let runs_alone = module
        .functions
        .iter()
        .any(|function| function.name.as_str() == "main" && !function.foreign)
        || crate::ridl_surface::described(module)?.is_some();
    let Some(root) = installed_sysroot().filter(|_| runs_alone) else {
        return Ok(());
    };
    let path = klf_vm::std_bytecode_path(&root, language, module.target.pointer_width());
    klf_vm::link_std(program, &path).map(|_| ())
}

/// The std archive `root` carries for `target`, or `None` when the language
/// ships no std. A sysroot built with a std for other targets cannot link
/// this one.
fn carried_std(root: &Path, language: &str, target: &str) -> Result<Option<PathBuf>, String> {
    let archive = std_archive(root, language, target);
    if archive.exists() {
        return Ok(Some(archive));
    }
    let mut carried: Vec<String> = std::fs::read_dir(root.join("lib").join(language))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("libstd.a").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    carried.sort();
    if carried.is_empty() {
        return Ok(None);
    }
    Err(format!(
        "the toolchain at {} carries no `{language}` std for `{target}` (it has {}); rebuild it with `--targets` including `{target}`",
        root.display(),
        carried.join(", ")
    ))
}

/// `lib/<lang>/<target>/libstd.a` under a sysroot.
pub(crate) fn std_archive(root: &Path, language: &str, target: &str) -> PathBuf {
    root.join("lib")
        .join(language)
        .join(target)
        .join("libstd.a")
}

/// A directory for one output's intermediate files, removed with it.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Result<Self, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "klf-frontend-rt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        Ok(Self(directory))
    }

    /// The directory itself.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let path = self.0.join(name);
        std::fs::write(&path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
