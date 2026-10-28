//! What a program brings to its link beyond its own code (section 26): the C
//! shims that pass its aggregates by value, and the native libraries, C
//! sources and linker flags its foreign functions need, which the definition
//! records as `link.library`, `link.c_source` and `link.flag` module
//! attributes, and the files it reads at run time, recorded as
//! `link.resource`.

use crate::output::Scratch;
use crate::toolchain::{CToolchain, is_apple};
use klf_kir::Module;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A native library a program links: the attribute's value is its path.
pub const LIBRARY_ATTRIBUTE: &str = "link.library";
/// A flag handed to the linker as written.
pub const FLAG_ATTRIBUTE: &str = "link.flag";
/// A C source compiled for the target and linked: its path, then each flag
/// it is compiled with, separated by tabs.
pub const C_SOURCE_ATTRIBUTE: &str = "link.c_source";
/// A file the program reads at run time, put in place when it is built: the
/// file's path, then the path it is copied to, separated by a tab.
pub const RESOURCE_ATTRIBUTE: &str = "link.resource";

/// The object holding `module`'s C shims, compiled into `scratch`, or `None`
/// when nothing crosses by value. `defined` is the object symbol of a
/// function the module defines.
pub(crate) fn c_shim_object(
    module: &Module,
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
    defined: &dyn Fn(&str) -> String,
) -> Result<Option<PathBuf>, String> {
    let prefix = if is_apple(target) { "_" } else { "" };
    let Some(source) = klf_kir::c_shim_source(module, target, prefix, defined)? else {
        return Ok(None);
    };
    generated_object("c", &source, &[], toolchain, target, scratch).map(Some)
}

/// The object of a source the compiler generated, compiled for `target`
/// with `flags`. The source includes nothing, so its text, flags and
/// toolchain decide the object, which the KLF cache keeps under their
/// digest; without a cache it is compiled into `scratch`.
pub(crate) fn generated_object(
    extension: &str,
    text: &str,
    flags: &[&str],
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
) -> Result<PathBuf, String> {
    let mut key = Vec::new();
    let identity = toolchain.identity();
    for part in [target.as_bytes(), identity.as_slice(), extension.as_bytes()]
        .into_iter()
        .chain(flags.iter().map(|flag| flag.as_bytes()))
    {
        key.extend_from_slice(part);
        key.push(0);
    }
    key.extend_from_slice(text.as_bytes());
    let name = klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&key));
    let directory = match klf_target::cache_root().map(|root| root.join("c-objects")) {
        Some(directory) if std::fs::create_dir_all(&directory).is_ok() => directory,
        _ => scratch.path().to_path_buf(),
    };
    let object = directory.join(format!("{name}.o"));
    if object.exists() {
        return Ok(object);
    }
    let source = scratch.write(&format!("{name}.{extension}"), text.as_bytes())?;
    let building = directory.join(format!("{name}.{}.o", std::process::id()));
    let mut arguments: Vec<&std::ffi::OsStr> = vec!["-c".as_ref(), "-O2".as_ref()];
    arguments.extend(flags.iter().map(std::ffi::OsStr::new));
    arguments.extend([source.as_os_str(), "-o".as_ref(), building.as_os_str()]);
    toolchain.run(&arguments)?;
    std::fs::rename(&building, &object)
        .map_err(|error| format!("cannot write {}: {error}", object.display()))?;
    Ok(object)
}

/// The object of each C source `module` declares, compiled for `target`.
/// An object is kept in the KLF cache and reused while the source, its
/// flags and every header it included are unchanged; without a cache it is
/// compiled into `scratch`.
pub(crate) fn c_source_objects(
    module: &Module,
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
) -> Result<Vec<PathBuf>, String> {
    let mut objects = Vec::new();
    for attribute in &module.attributes {
        let Some(value) = attribute
            .value
            .as_ref()
            .filter(|_| attribute.name == C_SOURCE_ATTRIBUTE)
        else {
            continue;
        };
        let mut fields = value.split('\t');
        let source = PathBuf::from(fields.next().unwrap_or_default());
        let flags: Vec<&str> = fields.collect();
        objects.push(c_source_object(
            &source, &flags, toolchain, target, scratch,
        )?);
    }
    Ok(objects)
}

fn c_source_object(
    source: &Path,
    flags: &[&str],
    toolchain: &CToolchain,
    target: &str,
    scratch: &Scratch,
) -> Result<PathBuf, String> {
    let text = std::fs::read(source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    let mut key = Vec::new();
    let identity = toolchain.identity();
    for part in [
        target.as_bytes(),
        identity.as_slice(),
        source.as_os_str().as_encoded_bytes(),
    ] {
        key.extend_from_slice(part);
        key.push(0);
    }
    for flag in flags {
        key.extend_from_slice(flag.as_bytes());
        key.push(0);
    }
    key.extend_from_slice(&text);
    let name = klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&key));
    let directory = match klf_target::cache_root().map(|root| root.join("c-objects")) {
        Some(directory) if std::fs::create_dir_all(&directory).is_ok() => directory,
        _ => scratch.path().to_path_buf(),
    };
    let object = directory.join(format!("{name}.o"));
    let depfile = directory.join(format!("{name}.d"));
    let stamp = directory.join(format!("{name}.stamp"));
    if object.exists()
        && let (Ok(recorded), Some(current)) =
            (std::fs::read_to_string(&stamp), dependency_digest(&depfile))
        && recorded == current
    {
        return Ok(object);
    }
    let building = directory.join(format!("{name}.{}.o", std::process::id()));
    let mut arguments: Vec<&std::ffi::OsStr> = vec!["-c".as_ref(), "-O2".as_ref()];
    arguments.extend(flags.iter().map(std::ffi::OsStr::new));
    arguments.extend([
        "-MD".as_ref(),
        "-MF".as_ref(),
        depfile.as_os_str(),
        source.as_os_str(),
        "-o".as_ref(),
        building.as_os_str(),
    ]);
    toolchain.run(&arguments)?;
    std::fs::rename(&building, &object)
        .map_err(|error| format!("cannot write {}: {error}", object.display()))?;
    let digest =
        dependency_digest(&depfile).ok_or_else(|| format!("cannot read {}", depfile.display()))?;
    std::fs::write(&stamp, digest)
        .map_err(|error| format!("cannot write {}: {error}", stamp.display()))?;
    Ok(object)
}

/// The digest of every file the depfile names, in its order, or `None` when
/// the depfile or one of them cannot be read.
fn dependency_digest(depfile: &Path) -> Option<String> {
    let text = std::fs::read_to_string(depfile).ok()?;
    let mut contents = Vec::new();
    for dependency in depfile_paths(&text) {
        klf_services::note_access(Path::new(&dependency));
        contents.extend_from_slice(dependency.as_bytes());
        contents.push(0);
        contents.extend_from_slice(&std::fs::read(&dependency).ok()?);
    }
    Some(klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&contents)))
}

/// The prerequisites of a make-style depfile: every path after the target's
/// `:`, with `\ ` an escaped space and `\` at a line end a continuation.
fn depfile_paths(text: &str) -> Vec<String> {
    let body = text.split_once(": ").map_or(text, |(_, body)| body);
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' if characters.peek() == Some(&' ') => {
                current.push(' ');
                characters.next();
            }
            '\\' if matches!(characters.peek(), Some('\n' | '\r')) => {}
            ' ' | '\n' | '\r' | '\t' => {
                if !current.is_empty() {
                    paths.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    paths
}

/// The libraries and flags `module` declares, in the order it declares them.
/// A shared library is also searched for at run time where it was linked.
pub(crate) fn link_arguments(module: &Module, target: &str) -> Vec<OsString> {
    let mut arguments = Vec::new();
    for attribute in &module.attributes {
        let Some(value) = &attribute.value else {
            continue;
        };
        match attribute.name.as_str() {
            LIBRARY_ATTRIBUTE if value.ends_with(".klbc") => {
                arguments.extend(image_library(value, target));
            }
            LIBRARY_ATTRIBUTE => {
                let path = Path::new(value);
                let shared = path
                    .extension()
                    .is_some_and(|extension| extension == "so" || extension == "dylib");
                if shared
                    && !target.contains("windows")
                    && let Some(rpath) = search_path(path)
                {
                    arguments.push(rpath);
                }
                arguments.push(OsString::from(value));
            }
            FLAG_ATTRIBUTE => arguments.push(OsString::from(value)),
            _ => {}
        }
    }
    arguments
}

/// The run-time search path that finds the shared library at `library`
/// where it was linked. A relative directory would be searched from wherever
/// the program is started, so it is made absolute.
pub(crate) fn search_path(library: &Path) -> Option<OsString> {
    let directory = library.parent()?;
    let directory = std::path::absolute(directory).unwrap_or_else(|_| directory.to_owned());
    let mut rpath = OsString::from("-Wl,-rpath,");
    rpath.push(directory.as_os_str());
    Some(rpath)
}

/// How a program links the `.klbc` image at `image`, a RIDL module run on
/// the VM: through the native library beside it, found again from the
/// directory it sits in. Loading the library runs the image.
pub(crate) fn image_library(image: &str, target: &str) -> Vec<OsString> {
    let image = std::path::absolute(image).unwrap_or_else(|_| image.into());
    let mut library = image.clone().into_os_string();
    if klf_target::is_msvc(target) {
        // A DLL links through its import library; the loader finds the DLL
        // itself beside the program or on `PATH`.
        library.push(".natives.lib");
        return vec![library];
    }
    library.push(if is_apple(target) {
        ".natives.dylib"
    } else {
        ".natives.so"
    });
    let mut arguments = Vec::new();
    if let Some(directory) = image.parent() {
        let mut rpath = OsString::from("-Wl,-rpath,");
        rpath.push(directory.as_os_str());
        arguments.push(rpath);
    }
    arguments.push(library);
    arguments
}

/// Puts each `.klbc` image `module` links, with its native library, beside
/// `binary` on Windows, which has no run-time search path: its loader looks
/// for a DLL a binary needs in the binary's own directory, and the library
/// runs the image it finds beside itself.
pub(crate) fn place_image_libraries(module: &Module, target: &str, binary: &Path) -> Result<(), String> {
    if !target.contains("windows") {
        return Ok(());
    }
    let Some(directory) = binary.parent() else {
        return Ok(());
    };
    for attribute in &module.attributes {
        let Some(value) = attribute.value.as_ref().filter(|value| value.ends_with(".klbc")) else {
            continue;
        };
        if attribute.name != LIBRARY_ATTRIBUTE {
            continue;
        }
        let image = std::path::absolute(value).unwrap_or_else(|_| value.into());
        let mut library = image.clone().into_os_string();
        library.push(".natives.dll");
        for file in [image, PathBuf::from(library)] {
            let Some(name) = file.file_name() else {
                continue;
            };
            let placed = directory.join(name);
            if std::path::absolute(&placed).ok() == Some(file.clone()) {
                continue;
            }
            std::fs::copy(&file, &placed)
                .map_err(|error| format!("cannot copy {} beside {}: {error}", file.display(), binary.display()))?;
        }
    }
    Ok(())
}

/// Copies each file `module` declares as a resource to where it says,
/// creating the directories on the way.
pub(crate) fn stage_resources(module: &Module) -> Result<(), String> {
    for attribute in &module.attributes {
        let Some(value) = attribute
            .value
            .as_ref()
            .filter(|_| attribute.name == RESOURCE_ATTRIBUTE)
        else {
            continue;
        };
        let Some((source, destination)) = value.split_once('\t') else {
            return Err(format!("resource `{value}` names no destination"));
        };
        let destination = Path::new(destination);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        copy_resource(Path::new(source), destination)?;
    }
    Ok(())
}

/// Copies a resource file, or a resource folder and everything in it: a
/// document such as a `.tessicon` is a folder.
fn copy_resource(source: &Path, destination: &Path) -> Result<(), String> {
    if source.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
        let entries = std::fs::read_dir(source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot read {}: {error}", source.display()))?;
            copy_resource(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(source, destination).map_err(|error| {
        format!("cannot copy {} to {}: {error}", source.display(), destination.display())
    })?;
    Ok(())
}

/// Gathers the DWARF of a linked Mach-O binary, which the linker leaves in
/// the objects it read, into `<binary>.dSYM` beside it, where LLDB and a
/// trap's backtrace find it (section 16.3). The objects are deleted once the
/// build ends, so this runs while they are still there. A binary whose
/// debug information cannot be gathered still runs: it says so and goes on.
pub(crate) fn gather_debug_information(binary: &std::path::Path) {
    let tool = crate::toolchain::find_tool("dsymutil").unwrap_or_else(|| "dsymutil".into());
    let bundle = std::path::PathBuf::from(format!("{}.dSYM", binary.display()));
    let gathered = std::process::Command::new(&tool)
        .arg(binary)
        .arg("-o")
        .arg(&bundle)
        .output();
    match gathered {
        Ok(result) if result.status.success() => {}
        Ok(result) => klf_diag::err!(
            "warning: the debug information of {} could not be gathered: {}",
            binary.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        ),
        Err(error) => klf_diag::err!(
            "warning: `{}` could not run to gather the debug information of {}: {error}",
            tool.display(),
            binary.display()
        ),
    }
}
