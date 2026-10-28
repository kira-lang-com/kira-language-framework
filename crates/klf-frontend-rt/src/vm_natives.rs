//! The native half of a `.klbc` (section 15.6): one shared library beside
//! the image holding what the program's foreign functions need natively —
//! its C sources, the libraries and frameworks it links, the C shims of its
//! by-value crossings — and the entries C calls its bytecode functions
//! through. The image imports it as a library, which the runner loads before
//! the program starts, binding it to the program's instance.
//!
//! The library speaks RIDL for the image (sections 20 to 24). A module with a
//! described surface gets the `ridlcc` entry of every export, each calling
//! the export's entry, and a constructor that runs the image in an instance
//! of its own when the library loads; a program links the library, natively
//! or from bytecode, as it links a native module's. A program importing
//! through RIDL gets the adapters its `call.foreign`s reach. A `.klbc` a
//! program names as a library is linked through its native library.

use crate::native_link::{
    C_SOURCE_ATTRIBUTE, FLAG_ATTRIBUTE, LIBRARY_ATTRIBUTE, c_shim_object, c_source_objects,
    image_library, search_path,
};
use crate::output::Scratch;
use crate::ridl_imports::Imported;
use crate::ridl_surface::Surface;
use crate::toolchain::{CToolchain, is_apple};
use klf_kir::{Function, Linkage, Module, Type};
use klf_vm::{
    ForeignClass, Import, ImportKind, Program, entered_from_c, foreign_class, vm_entry_symbol,
};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Builds the native library `program` needs beside `output` and imports it,
/// or leaves the program alone when it needs nothing native.
pub(crate) fn emit_natives(
    module: &Module,
    program: &mut Program,
    output: &Path,
) -> Result<(), String> {
    let surface = crate::ridl_surface::described(module)?;
    let imported = crate::ridl_imports::imported(module)?;
    let declares_native = module.attributes.iter().any(|attribute| {
        [C_SOURCE_ATTRIBUTE, LIBRARY_ATTRIBUTE, FLAG_ATTRIBUTE].contains(&attribute.name.as_str())
    });
    let crosses = module.functions.iter().any(|function| {
        (function.c_prototype.is_some() && (function.foreign || entered_from_c(function)))
            || entered_natively(function)
    });
    if !declares_native && !crosses && surface.is_none() && imported.is_empty() {
        return Ok(());
    }
    let target = module.target.triple().to_owned();
    let toolchain = CToolchain::for_target(&target)?;
    let scratch = Scratch::new()?;
    let described = surface.as_ref().map(|surface| surface.module.name.as_str());
    let defined = |name: &str| {
        module
            .functions
            .iter()
            .find(|function| function.name.as_str() == name)
            .and_then(|function| entry_symbol(function, described))
            .unwrap_or_else(|| vm_entry_symbol(name))
    };
    let mut objects = Vec::new();
    if let Some(shims) = c_shim_object(module, &toolchain, &target, &scratch, &|name| {
        vm_entry_symbol(name)
    })? {
        objects.push(shims);
    }
    let entries = entry_source(
        module,
        if is_apple(&target) { "_" } else { "" },
        described,
        surface.is_some(),
    );
    let source = scratch.write("vm_entries.c", entries.as_bytes())?;
    objects.push(compiled(&toolchain, &source)?);
    objects.extend(ridl_objects(
        surface.as_ref(),
        &imported,
        &toolchain,
        &target,
        &scratch,
        &defined,
    )?);
    objects.extend(c_source_objects(module, &toolchain, &target, &scratch)?);

    let name = format!(
        "{}.natives.{}",
        output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        if is_apple(&target) { "dylib" } else { "so" }
    );
    let library = output.with_file_name(&name);
    if let Some(directory) = library
        .parent()
        .filter(|directory| !directory.as_os_str().is_empty())
    {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    }
    let mut arguments: Vec<OsString> = Vec::new();
    if is_apple(&target) {
        arguments.extend(["-dynamiclib", "-undefined", "dynamic_lookup"].map(OsString::from));
        arguments.push(format!("-Wl,-install_name,@rpath/{name}").into());
    } else {
        // Every program's library defines the same helper names; each binds
        // its own, so a library loaded beside another never calls into it.
        arguments.extend(["-shared", "-fPIC", "-Wl,-Bsymbolic"].map(OsString::from));
        arguments.push(format!("-Wl,-soname,{name}").into());
    }
    arguments.extend(objects.into_iter().map(OsString::from));
    arguments.extend(whole_libraries(module, &target));
    arguments.push("-o".into());
    arguments.push(library.clone().into());
    let arguments: Vec<&std::ffi::OsStr> = arguments.iter().map(OsString::as_os_str).collect();
    toolchain.run(&arguments)?;
    program.imports.push(Import {
        symbol: name,
        kind: ImportKind::Library { first: 0 },
    });
    Ok(())
}

/// The objects of the `ridlcc` entries of `surface` and of the adapters for
/// what the program imports, compiled from LLVM IR by the bundled clang.
fn ridl_objects(
    surface: Option<&Surface>,
    imported: &[Imported],
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
    defined: &dyn Fn(&str) -> String,
) -> Result<Vec<PathBuf>, String> {
    let mut sources = Vec::new();
    if let Some(surface) = surface {
        let text = crate::ridl_entries::source(surface, target, defined)?;
        sources.push(scratch.write("ridl.ll", text.as_bytes())?);
    }
    for (index, imported) in imported.iter().enumerate() {
        let text = crate::ridl_imports::source(imported, target, defined)?;
        sources.push(scratch.write(&format!("ridl-import-{index}.ll"), text.as_bytes())?);
    }
    sources
        .iter()
        .map(|source| compiled(toolchain, source))
        .collect()
}

/// `source`, C or LLVM IR, compiled to a position-independent object beside
/// it.
fn compiled(toolchain: &CToolchain, source: &Path) -> Result<PathBuf, String> {
    let object = source.with_extension("o");
    toolchain.run(&[
        "-c".as_ref(),
        "-O2".as_ref(),
        "-fPIC".as_ref(),
        "-Wno-override-module".as_ref(),
        source.as_os_str(),
        "-o".as_ref(),
        object.as_os_str(),
    ])?;
    Ok(object)
}

/// Whether native code calls `function` by its name: a library's export.
fn entered_natively(function: &Function) -> bool {
    !function.foreign
        && function.linkage == Linkage::Exported
        && !function.blocks.is_empty()
        && function.name.as_str() != "main"
}

/// The symbol of the entry C calls `function` through, if it has one. A
/// module with a described surface keeps its exports under
/// `<module>.<name>`, as its native library does, so two modules' functions
/// never share a symbol.
fn entry_symbol(function: &Function, described: Option<&str>) -> Option<String> {
    if entered_natively(function) {
        Some(match described {
            Some(module) => format!("{module}.{}", function.name),
            None => function.name.to_string(),
        })
    } else if entered_from_c(function) {
        Some(vm_entry_symbol(function.name.as_str()))
    } else {
        None
    }
}

/// The libraries and flags `module` links, every static archive taken whole:
/// the VM finds a symbol by name, so nothing in the library references the
/// archive members a program calls.
fn whole_libraries(module: &Module, target: &str) -> Vec<OsString> {
    let mut arguments = Vec::new();
    for attribute in &module.attributes {
        let Some(value) = &attribute.value else {
            continue;
        };
        if attribute.name != LIBRARY_ATTRIBUTE && attribute.name != FLAG_ATTRIBUTE {
            continue;
        }
        if attribute.name == LIBRARY_ATTRIBUTE && value.ends_with(".klbc") {
            arguments.extend(image_library(value, target));
            continue;
        }
        if value.ends_with(".a") && !value.starts_with('-') {
            if is_apple(target) {
                arguments.push(format!("-Wl,-force_load,{value}").into());
            } else {
                arguments.push(format!("-Wl,--whole-archive,{value},--no-whole-archive").into());
            }
            continue;
        }
        if attribute.name == LIBRARY_ATTRIBUTE
            && (value.ends_with(".so") || value.ends_with(".dylib"))
            && let Some(rpath) = search_path(Path::new(value))
        {
            arguments.push(rpath);
        }
        arguments.push(value.into());
    }
    arguments
}

/// The C source of the entry of every bytecode function C calls, and of the
/// binding through which the library learns the instance its functions run
/// on. An entry takes the function's C signature, passes its arguments as
/// words to `klfvm_invoke` with the instance and the function's index, and
/// returns its result. A RIDL module's library also runs its image when it
/// loads.
fn entry_source(module: &Module, prefix: &str, described: Option<&str>, attaches: bool) -> String {
    let mut source = String::from(
        "typedef __UINT64_TYPE__ uint64_t;\ntypedef __UINT32_TYPE__ uint32_t;\n\
         extern void klfvm_invoke(const void *, uint32_t, const uint64_t *, uint64_t *);\n\
         static const void *klf_vm_instance;\nstatic uint32_t klf_vm_first;\n\
         void klf_vm_bind(const void *instance, uint32_t first) {\n\
         klf_vm_instance = instance;\nklf_vm_first = first;\n}\n",
    );
    if attaches {
        source.push_str(
            "extern void klfvm_attach(const void *);\n\
             __attribute__((constructor)) static void klf_vm_attach(void) { klfvm_attach((const void *)&klf_vm_attach); }\n",
        );
    }
    for (index, function) in module.functions.iter().enumerate() {
        if let Some(symbol) = entry_symbol(function, described) {
            write_entry(&mut source, index, function, prefix, &symbol);
        }
    }
    source
}

fn write_entry(source: &mut String, index: usize, function: &Function, prefix: &str, symbol: &str) {
    let prototype = function.c_prototype.as_ref();
    let classes: Vec<ForeignClass> = function
        .parameters
        .iter()
        .enumerate()
        .map(|(position, parameter)| {
            foreign_class(
                &parameter.ty,
                prototype.and_then(|prototype| prototype.parameters.get(position)),
            )
        })
        .collect();
    let result = match &function.result {
        Type::Unit | Type::Never => None,
        ty => Some(foreign_class(
            ty,
            prototype.map(|prototype| &prototype.result),
        )),
    };
    let parameters: Vec<String> = classes
        .iter()
        .enumerate()
        .map(|(position, class)| format!("{} a{position}", c_type(*class)))
        .collect();
    let returned = result.map_or("void", c_type);
    let parameter_list = if parameters.is_empty() {
        "void".to_owned()
    } else {
        parameters.join(", ")
    };
    let _ = writeln!(
        source,
        "{returned} klf_vm_entry{index}({parameter_list}) __asm__(\"{prefix}{symbol}\");"
    );
    let _ = writeln!(
        source,
        "{returned} klf_vm_entry{index}({parameter_list}) {{"
    );
    let count = classes.len().max(1);
    let _ = writeln!(source, "    uint64_t words[{count}] = {{0}};");
    for (position, class) in classes.iter().enumerate() {
        let _ = match class {
            ForeignClass::F32 => writeln!(
                source,
                "    __builtin_memcpy(&words[{position}], &a{position}, 4);"
            ),
            ForeignClass::F64 => writeln!(
                source,
                "    __builtin_memcpy(&words[{position}], &a{position}, 8);"
            ),
            _ => writeln!(source, "    words[{position}] = (uint64_t)a{position};"),
        };
    }
    let _ = writeln!(
        source,
        "    uint64_t word = 0;\n    klfvm_invoke(klf_vm_instance, klf_vm_first + {index}u, words, &word);"
    );
    let _ = match result {
        None => writeln!(source, "}}"),
        Some(ForeignClass::F32) => writeln!(
            source,
            "    float value;\n    __builtin_memcpy(&value, &word, 4);\n    return value;\n}}"
        ),
        Some(ForeignClass::F64) => writeln!(
            source,
            "    double value;\n    __builtin_memcpy(&value, &word, 8);\n    return value;\n}}"
        ),
        Some(class) => writeln!(source, "    return ({})word;\n}}", c_type(class)),
    };
}

fn c_type(class: ForeignClass) -> &'static str {
    match class {
        ForeignClass::Signed(bits) if bits <= 8 => "__INT8_TYPE__",
        ForeignClass::Signed(bits) if bits <= 16 => "__INT16_TYPE__",
        ForeignClass::Signed(bits) if bits <= 32 => "__INT32_TYPE__",
        ForeignClass::Signed(_) => "__INT64_TYPE__",
        ForeignClass::Unsigned(bits) if bits <= 8 => "__UINT8_TYPE__",
        ForeignClass::Unsigned(bits) if bits <= 16 => "__UINT16_TYPE__",
        ForeignClass::Unsigned(bits) if bits <= 32 => "__UINT32_TYPE__",
        ForeignClass::Unsigned(_) => "__UINT64_TYPE__",
        ForeignClass::F32 => "float",
        ForeignClass::F64 => "double",
    }
}
