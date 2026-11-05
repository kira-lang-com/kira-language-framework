use super::*;
use std::collections::HashSet;

#[allow(clippy::too_many_arguments)]
pub(super) fn append_languages(
    session: &mut Session,
    target: &Target,
    languages: &[&Path],
    profile: Profile,
    compiler: &std::ffi::OsStr,
    home: &Path,
    library: &Path,
    scratch: &Scratch,
    seen: &mut HashSet<PathBuf>,
    modules: &mut HashSet<String>,
    objects: &mut Vec<PathBuf>,
    archives: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for (offset, language) in languages.iter().enumerate() {
        let canonical = language
            .canonicalize()
            .map_err(|error| format!("{}: {error}", language.display()))?;
        if !seen.insert(canonical) {
            return Err(format!(
                "`{}` names a language already linked into this frontend",
                language.display()
            ));
        }
        let index = offset + 1;
        if is_package(language) {
            let (other, metadata, module) = frontend_metadata(session, language, target)?;
            if other.shader.is_none() {
                return Err(format!("{} is not a shader language", language.display()));
            }
            if !modules.insert(module.clone()) {
                return Err(format!(
                    "Kira frontend moduleRoot `{module}` is already linked into this driver"
                ));
            }
            let (component_objects, archive) = compile_component(
                language, &module, &metadata, index, compiler, home, library, profile, scratch,
            )?;
            objects.extend(component_objects);
            archives.push(archive);
        } else {
            let object =
                crate::frontend::compile_shader_out(session, language, index, target, profile)?;
            objects.push(scratch.write(&format!("shader-{index}.o"), &object)?);
        }
    }
    Ok(())
}

pub(super) fn link_components(
    scratch: &Scratch,
    objects: &mut Vec<PathBuf>,
    archives: &mut Vec<PathBuf>,
    count: usize,
    binary: &Path,
) -> Result<(), String> {
    let registry = klf_cranelift::emit_frontend_registry(crate::host_triple(), count)
        .map_err(|error| error.to_string())?;
    objects.push(scratch.write("registry.o", &registry)?);
    objects.push(crate::kernel_ridl::compile_entries(
        &scratch.0,
        crate::host_triple(),
    )?);
    archives.push(crate::kernel_archive(
        crate::KernelArchive::Driver,
        crate::host_triple(),
    )?);
    crate::kernel::link(objects, archives, binary, crate::host_triple())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compile_component(
    root: &Path,
    module_name: &str,
    metadata: &FrontendMetadata,
    index: usize,
    compiler: &std::ffi::OsStr,
    home: &Path,
    library: &Path,
    profile: Profile,
    scratch: &Scratch,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    let mut metadata = metadata.clone();
    let stem = format!("KLFFrontend_{index}");
    let archive = scratch.0.join(format!("{stem}.a"));
    let contract_path = scratch.0.join(format!("{stem}.ridl"));
    compile_kira(compiler, root, home, library, "lib", &archive, profile)?;
    let archive_bytes = std::fs::read(&archive)
        .map_err(|error| format!("cannot read frontend archive: {error}"))?;
    metadata.definition = klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&archive_bytes));
    compile_kira(
        compiler,
        root,
        home,
        library,
        "ridl",
        &contract_path,
        profile,
    )?;
    let contract_text = std::fs::read_to_string(&contract_path)
        .map_err(|error| format!("cannot read frontend contract: {error}"))?;
    let contract = klf_ridl::parse(FileId::from_u32(0), module_name, &contract_text)
        .map_err(|diagnostic| format!("invalid frontend RIDL contract: {diagnostic}"))?;
    let has_format = metadata.declares(klf_defsvc::Hook::Format);
    let has_compat = metadata.declares(klf_defsvc::Hook::CompatDriver);
    let has_run = metadata.declares(klf_defsvc::Hook::RunProgram);
    let has_project = metadata.declares(klf_defsvc::Hook::Project);
    let has_emit = metadata.declares(klf_defsvc::Hook::Emit);
    validate_entries(
        &contract,
        has_format,
        has_compat,
        has_run,
        has_project,
        has_emit,
    )?;
    let callers = entries(has_format, has_compat, has_run, has_project, has_emit)
        .into_iter()
        .map(|(name, _)| {
            (
                name.to_owned(),
                klf_ridl::bindgen::caller_symbol(module_name, name),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let adapters = klf_ridl::llvm::adapters(
        &contract,
        &callers,
        crate::host_triple(),
        &str::to_owned,
        false,
    )?;
    let wrapper = trampoline(
        &metadata.encode(),
        has_format,
        has_compat,
        has_run,
        has_project,
        has_emit,
    )
    .replace(
        "KLFFrontend_klfFrontend",
        &format!("{module_name}_klfFrontend"),
    )
    .replace("__klf_init_KLFFrontend", &format!("__klf_init_{stem}"))
    .replace("klffe_metadata_0", &format!("klffe_metadata_{index}"))
    .replace("klffe_phase_0", &format!("klffe_phase_{index}"))
    .replace("klffe_init_0", &format!("klffe_init_{index}"));
    let wrapper_path = compile_source(scratch, &format!("frontend-{index}.c"), &wrapper)?;
    let adapter_path = compile_source(scratch, &format!("adapters-{index}.ll"), &adapters)?;
    Ok((vec![wrapper_path, adapter_path], archive))
}

pub(super) fn compile_kira(
    compiler: &std::ffi::OsStr,
    root: &Path,
    home: &Path,
    library: &Path,
    emit: &str,
    output: &Path,
    profile: Profile,
) -> Result<(), String> {
    let dependency = format!("KLF={}", library.to_string_lossy().replace('\\', "/"));
    let mut command = Command::new(compiler);
    command
        .arg("build")
        .arg(root)
        .args([
            "--dependency",
            &dependency,
            "--emit",
            emit,
            "--target",
            crate::host_triple(),
            "-o",
        ])
        .arg(output)
        .env("KLF_HOME", home)
        .env("KLF_FRONTEND_RIDL_RUNTIME", "1");
    if profile == Profile::Release {
        command.arg("--optimize");
    }
    let result = command
        .output()
        .map_err(|error| format!("cannot start Kira compiler: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "Kira frontend compilation failed\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}
pub(super) fn entries(
    has_format: bool,
    has_compat: bool,
    has_run: bool,
    has_project: bool,
    has_emit: bool,
) -> Vec<(&'static str, usize)> {
    let mut entries = ENTRIES.to_vec();
    if has_format {
        entries.push(("klfFrontendFormat", 3));
    }
    if has_compat {
        entries.push(("klfFrontendCompatDriver", 1));
    }
    if has_run {
        entries.push(("klfFrontendRunProgram", 2));
    }
    if has_project {
        entries.push(("klfFrontendProject", 2));
    }
    if has_emit {
        entries.push(("klfFrontendEmit", 1));
    }
    entries
}
