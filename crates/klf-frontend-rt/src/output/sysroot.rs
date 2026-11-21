//! The sysroot a driver is installed in and the std it carries.

use klf_defsvc::FrontendMetadata;
use klf_kir::Module;
use std::path::{Path, PathBuf};

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
pub(super) fn carried_std(
    root: &Path,
    language: &str,
    target: &str,
) -> Result<Option<PathBuf>, String> {
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
fn std_archive(root: &Path, language: &str, target: &str) -> PathBuf {
    root.join("lib")
        .join(language)
        .join(target)
        .join("libstd.a")
}
