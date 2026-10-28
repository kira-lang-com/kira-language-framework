//! `klf build <defdir> --out <dir>` for a `ShaderLanguage` (section 5.6):
//! the shader driver, the language's shared library modules, and the lock.
//! Shaders link nothing, so there is no std to compile and no runtime.

use crate::build_frontend;
use crate::kernel::host_triple;
use crate::sysroot::{BuiltSysroot, collect_all, create};
use klf_services::{DefinitionPackage, LanguageDescriptor, Session};
use klf_target::Target;
use std::path::{Path, PathBuf};

pub(crate) fn assemble(
    session: &mut Session,
    package: &Path,
    loaded: &DefinitionPackage,
    descriptor: &LanguageDescriptor,
    staging: &Path,
    languages: &[&Path],
    profile: crate::Profile,
) -> Result<BuiltSysroot, String> {
    let Some(family) = &descriptor.shader else {
        return Err(format!("{} is not a shader language", descriptor.name));
    };
    let host = host_triple();
    let host_target: Target = host
        .parse()
        .map_err(|_| format!("klf runs on an unsupported host `{host}`"))?;
    create(&staging.join("bin"))?;
    let driver_name = if descriptor.driver.is_empty() {
        descriptor.name.clone()
    } else {
        descriptor.driver.clone()
    };
    let driver = staging.join("bin").join(&driver_name);
    build_frontend(
        session,
        package,
        Some(&driver),
        &host_target,
        languages,
        profile,
    )?;

    let mut files = Vec::new();
    let library = package.join(&family.library_root);
    if !family.library_root.is_empty() && library.is_dir() {
        collect_all(&library, &mut files)?;
        files.sort();
        let to = staging.join("share").join(&descriptor.name).join("library");
        for file in &files {
            copy_into(&library, file, &to)?;
        }
    }
    let targets: Vec<&str> = family.targets.iter().map(|target| target.name()).collect();
    let lock = crate::lock::definition_lock(loaded, &library, files.iter(), "library", &targets)?;
    std::fs::write(staging.join("definition.lock"), lock)
        .map_err(|error| format!("cannot write definition.lock: {error}"))?;
    Ok(BuiltSysroot {
        root: staging.to_path_buf(),
        driver,
        runner: PathBuf::new(),
        std_libraries: Vec::new(),
    })
}

fn copy_into(root: &Path, file: &Path, to: &Path) -> Result<(), String> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", file.display(), root.display()))?;
    let target = to.join(relative);
    if target == file {
        return Err(format!("{} would be copied onto itself", file.display()));
    }
    if let Some(parent) = target.parent() {
        create(parent)?;
    }
    std::fs::copy(file, &target)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", file.display()))
}
