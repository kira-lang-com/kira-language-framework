//! Building a Kira-written frontend into the kernel's native driver.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use klf_core::FileId;
use klf_defsvc::FrontendMetadata;
use klf_ridl::module::{Convention, ItemKind, Module};
use klf_ridl::types::{Scalar, Type};
use klf_services::{DefinitionPackage, DefinitionSource, Session};
use klf_target::Target;

use crate::{BuiltFrontend, Profile};

const ENTRIES: [(&str, usize); 6] = [
    ("klfFrontendUnits", 2),
    ("klfFrontendParse", 3),
    ("klfFrontendElaborate", 3),
    ("klfFrontendLower", 3),
    ("klfFrontendBegin", 0),
    ("klfFrontendDispose", 0),
];

pub(crate) fn build(
    session: &mut Session,
    root: &Path,
    output: Option<&Path>,
    target: &Target,
    languages: &[&Path],
    profile: Profile,
) -> Result<BuiltFrontend, String> {
    let (descriptor, metadata) = frontend_metadata(session, root, target)?;
    if descriptor.shader.is_none() && !languages.is_empty() {
        return Err("additional language packages require a shader frontend".to_owned());
    }
    build_library(session, root, output, target, languages, profile, &descriptor, &metadata)
}

fn frontend_metadata(
    session: &mut Session, root: &Path, target: &Target,
) -> Result<(klf_services::LanguageDescriptor, FrontendMetadata), String> {
    let (package, definition) = read_package(session, root)?;
    let descriptor = &definition.descriptor;
    let mut metadata = FrontendMetadata::from_descriptor(descriptor, target.triple());
    metadata.materializes = [true; 4];
    if definition.entrypoints.emit.is_some() {
        metadata.declare(klf_defsvc::Hook::Emit);
    }
    if definition.entrypoints.format.is_some() {
        metadata.declare(klf_defsvc::Hook::Format);
    }
    if definition.entrypoints.compat_driver.is_some() {
        metadata.declare(klf_defsvc::Hook::CompatDriver);
    }
    if definition.entrypoints.run_program.is_some() {
        metadata.declare(klf_defsvc::Hook::RunProgram);
    }
    if definition.entrypoints.project.is_some() {
        metadata.declare(klf_defsvc::Hook::Project);
    }
    let library = descriptor.shader.as_ref().map_or(descriptor.std_root.as_str(), |shader| shader.library_root.as_str());
    if !library.is_empty() {
        let std_root = root.join(library);
        metadata.std_root = klf_target::plain_path(std_root.canonicalize().unwrap_or(std_root))
            .to_string_lossy()
            .into_owned();
    }
    metadata.definition = definition_hash(&package);
    if descriptor.shader.is_none() { klf_defsvc::driver_cli::validate(&metadata)?; }

    Ok((definition.descriptor, metadata))
}

/// Whether a package supplies a Kira-written language frontend.
#[must_use]
pub fn is_package(root: &Path) -> bool {
    root.join("package.kira").is_file() && root.join("app/Language.kira").is_file()
}

/// Reads the Kira frontend's declared language metadata.
///
/// # Errors
/// Returns an error when the declaration cannot be read or validated.
pub fn descriptor(session: &mut Session, root: &Path) -> Result<klf_services::LanguageDescriptor, String> {
    read_package(session, root).map(|(_, definition)| definition.descriptor)
}

/// Type-checks a Kira frontend and verifies its RIDL phase entries.
///
/// # Errors
/// Returns a diagnostic if the metadata, compiler, or phase contract is invalid.
pub fn check(session: &mut Session, root: &Path, target: &Target) -> Result<(), String> {
    let (_, definition) = read_package(session, root)?;
    let module_name = package_module(session, root)?;
    if definition.descriptor.shader.is_none() {
        klf_defsvc::driver_cli::validate(&FrontendMetadata::from_descriptor(&definition.descriptor, target.triple()))?;
    }
    let home = crate::kernel::archive_home().ok_or("no KLF distribution for the Kira frontend")?;
    let library = home.join("share/klf/KLF");
    if !library.join("package.kira").is_file() {
        return Err(format!("{} does not contain the KLF Kira package", home.display()));
    }
    let compiler = std::env::var_os("KLF_KIRA").unwrap_or_else(|| "kira".into());
    let scratch = Scratch::new()?;
    let contract_path = scratch.0.join("KLFFrontend.ridl");
    compile_kira(&compiler, root, &home, &library, "ridl", &contract_path, Profile::Debug)?;
    let text = std::fs::read_to_string(&contract_path).map_err(|error| format!("cannot read frontend contract: {error}"))?;
    let module = klf_ridl::parse(FileId::from_u32(0), &module_name, &text)
        .map_err(|diagnostic| format!("invalid frontend RIDL contract: {diagnostic}"))?;
    validate_entries(&module, definition.entrypoints.format.is_some(), definition.entrypoints.compat_driver.is_some(), definition.entrypoints.run_program.is_some(), definition.entrypoints.project.is_some(), definition.entrypoints.emit.is_some())
}

fn package_module(session: &mut Session, root: &Path) -> Result<String, String> {
    use klf_kira_syntax::{TokenKind, ast::DeclKind};
    let path = root.join("package.kira");
    let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let id = session.add_file(path.to_string_lossy().as_ref(), text);
    let parsed = klf_kira_syntax::parse(session.file(id));
    let construct = parsed.file.decls.iter().find_map(|decl| match &decl.kind {
        DeclKind::Construct(construct) => Some(construct), _ => None,
    }).ok_or_else(|| format!("{} has no package construct", path.display()))?;
    let mut module = construct.name.name.as_str().to_owned();
    // Package manifests use Kira's struct-literal syntax, which the KCore
    // parser does not accept. Read this top-level field from tokens so a
    // dependency declared before it cannot hide it during parser recovery.
    let source = session.file(id);
    let tokens = klf_kira_syntax::lex(source).tokens;
    let text = source.text();
    let spelling = |token: &klf_kira_syntax::Token| &text[token.span.start as usize..token.span.end as usize];
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => depth = depth.saturating_sub(1),
            TokenKind::Let if depth == 1 && tokens.get(index + 1).is_some_and(|name| spelling(name) == "moduleRoot") => {
                let rest = &tokens[index + 2..];
                let equals = rest.iter().position(|item| item.kind == TokenKind::Eq)
                    .ok_or("package moduleRoot needs a string literal")?;
                let value = rest.get(equals + 1).filter(|item| item.kind == TokenKind::Str)
                    .ok_or("package moduleRoot needs a string literal")?;
                module = spelling(value).strip_prefix('"').and_then(|value| value.strip_suffix('"'))
                    .ok_or("package moduleRoot needs a string literal")?.to_owned();
            }
            _ => {}
        }
    }
    if module.is_empty() || !module.bytes().enumerate().all(|(index, byte)| byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())) {
        return Err(format!("package moduleRoot `{module}` is not a valid frontend symbol"));
    }
    Ok(module)
}

pub(crate) fn read_package(
    session: &mut Session,
    root: &Path,
) -> Result<
    (
        DefinitionPackage,
        klf_kira_lower::LanguageDefinitionMetadata,
    ),
    String,
> {
    let source = root.join("app/Language.kira");
    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    let id = session.add_file(source.to_string_lossy().as_ref(), text);
    let parsed = klf_kira_syntax::parse(session.file(id));
    for diagnostic in parsed.diagnostics {
        session.report(diagnostic);
    }
    let definition = klf_kira_lower::extract_language_metadata(session, &parsed.file)
        .ok_or_else(|| format!("cannot read language metadata in {}", source.display()))?;
    let package = read_sources(root)?;
    Ok((package, definition))
}

fn build_library(
    session: &mut Session,
    root: &Path,
    output: Option<&Path>,
    target: &Target,
    languages: &[&Path],
    profile: Profile,
    descriptor: &klf_services::LanguageDescriptor,
    metadata: &FrontendMetadata,
) -> Result<BuiltFrontend, String> {
    let home = crate::kernel::archive_home().ok_or("no KLF distribution for the Kira frontend")?;
    let library = home.join("share/klf/KLF");
    if !library.join("package.kira").is_file() {
        return Err(format!("{} does not contain the KLF Kira package", home.display()));
    }
    let compiler = std::env::var_os("KLF_KIRA").unwrap_or_else(|| "kira".into());
    let scratch = Scratch::new()?;
    let mut modules = Vec::new();
    let mut objects = Vec::new();
    let mut archives = Vec::new();
    let mut seen = vec![root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?];
    let module = package_module(session, root)?;
    let (component_objects, archive) = compile_component(root, &module, metadata, 0, &compiler, &home, &library, profile, &scratch)?;
    modules.push(module);
    objects.extend(component_objects);
    archives.push(archive);
    append_languages(session, target, languages, profile, &compiler, &home, &library, &scratch,
        &mut seen, &mut modules, &mut objects, &mut archives)?;
    let binary = output.map_or_else(|| PathBuf::from(&descriptor.driver), Path::to_path_buf);
    link_components(&scratch, &mut objects, &mut archives, languages.len() + 1, &binary)?;
    Ok(BuiltFrontend { binary, driver: descriptor.driver.clone() })
}

/// Links Kira shader languages into a driver whose primary frontend is KCore.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_with_kcore_primary(
    session: &mut Session, root: &Path, primary: &[u8], binary: &Path, target: &Target,
    languages: &[&Path], profile: Profile,
) -> Result<(), String> {
    let home = crate::kernel::archive_home().ok_or("no KLF distribution for the Kira frontend")?;
    let library = home.join("share/klf/KLF");
    if !library.join("package.kira").is_file() {
        return Err(format!("{} does not contain the KLF Kira package", home.display()));
    }
    let compiler = std::env::var_os("KLF_KIRA").unwrap_or_else(|| "kira".into());
    let scratch = Scratch::new()?;
    let mut seen = vec![root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?];
    let mut modules = Vec::new();
    let mut objects = vec![scratch.write("primary.o", primary)?];
    let mut archives = Vec::new();
    append_languages(session, target, languages, profile, &compiler, &home, &library, &scratch,
        &mut seen, &mut modules, &mut objects, &mut archives)?;
    link_components(&scratch, &mut objects, &mut archives, languages.len() + 1, binary)
}

#[allow(clippy::too_many_arguments)]
fn append_languages(
    session: &mut Session, target: &Target, languages: &[&Path], profile: Profile,
    compiler: &std::ffi::OsStr, home: &Path, library: &Path, scratch: &Scratch,
    seen: &mut Vec<PathBuf>, modules: &mut Vec<String>, objects: &mut Vec<PathBuf>, archives: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for (offset, language) in languages.iter().enumerate() {
        let canonical = language.canonicalize().map_err(|error| format!("{}: {error}", language.display()))?;
        if seen.contains(&canonical) {
            return Err(format!("`{}` names a language already linked into this frontend", language.display()));
        }
        seen.push(canonical);
        let index = offset + 1;
        if is_package(language) {
            let (other, metadata) = frontend_metadata(session, language, target)?;
            if other.shader.is_none() { return Err(format!("{} is not a shader language", language.display())); }
            let module = package_module(session, language)?;
            if modules.contains(&module) {
                return Err(format!("Kira frontend moduleRoot `{module}` is already linked into this driver"));
            }
            let (component_objects, archive) = compile_component(language, &module, &metadata, index, compiler, home, library, profile, scratch)?;
            modules.push(module);
            objects.extend(component_objects);
            archives.push(archive);
        } else {
            let object = crate::frontend::compile_shader_out(session, language, index, target, profile)?;
            objects.push(scratch.write(&format!("shader-{index}.o"), &object)?);
        }
    }
    Ok(())
}

fn link_components(
    scratch: &Scratch, objects: &mut Vec<PathBuf>, archives: &mut Vec<PathBuf>,
    count: usize, binary: &Path,
) -> Result<(), String> {
    let registry = klf_cranelift::emit_frontend_registry(crate::host_triple(), count)
        .map_err(|error| error.to_string())?;
    objects.push(scratch.write("registry.o", &registry)?);
    objects.push(crate::kernel_ridl::compile_entries(&scratch.0, crate::host_triple())?);
    archives.push(crate::kernel_archive(crate::KernelArchive::Driver, crate::host_triple())?);
    crate::kernel::link(objects, archives, binary, crate::host_triple())
}

#[allow(clippy::too_many_arguments)]
fn compile_component(
    root: &Path, module_name: &str, metadata: &FrontendMetadata, index: usize,
    compiler: &std::ffi::OsStr, home: &Path, library: &Path, profile: Profile, scratch: &Scratch,
) -> Result<(Vec<PathBuf>, PathBuf), String> {
    let mut metadata = metadata.clone();
    let stem = format!("KLFFrontend_{index}");
    let archive = scratch.0.join(format!("{stem}.a"));
    let contract_path = scratch.0.join(format!("{stem}.ridl"));
    compile_kira(compiler, root, home, library, "lib", &archive, profile)?;
    let archive_bytes = std::fs::read(&archive).map_err(|error| format!("cannot read frontend archive: {error}"))?;
    metadata.definition = klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&archive_bytes));
    compile_kira(compiler, root, home, library, "ridl", &contract_path, profile)?;
    let contract_text = std::fs::read_to_string(&contract_path).map_err(|error| format!("cannot read frontend contract: {error}"))?;
    let contract = klf_ridl::parse(FileId::from_u32(0), module_name, &contract_text)
        .map_err(|diagnostic| format!("invalid frontend RIDL contract: {diagnostic}"))?;
    let has_format = metadata.declares(klf_defsvc::Hook::Format);
    let has_compat = metadata.declares(klf_defsvc::Hook::CompatDriver);
    let has_run = metadata.declares(klf_defsvc::Hook::RunProgram);
    let has_project = metadata.declares(klf_defsvc::Hook::Project);
    let has_emit = metadata.declares(klf_defsvc::Hook::Emit);
    validate_entries(&contract, has_format, has_compat, has_run, has_project, has_emit)?;
    let callers = entries(has_format, has_compat, has_run, has_project, has_emit).into_iter()
        .map(|(name, _)| (name.to_owned(), klf_ridl::bindgen::caller_symbol(module_name, name)))
        .collect::<BTreeMap<_, _>>();
    let adapters = klf_ridl::llvm::adapters(&contract, &callers, crate::host_triple(), &str::to_owned, false)?;
    let wrapper = trampoline(&metadata.encode(), has_format, has_compat, has_run, has_project, has_emit)
        .replace("KLFFrontend_klfFrontend", &format!("{module_name}_klfFrontend"))
        .replace("__klf_init_KLFFrontend", &format!("__klf_init_{stem}"))
        .replace("klffe_metadata_0", &format!("klffe_metadata_{index}"))
        .replace("klffe_phase_0", &format!("klffe_phase_{index}"))
        .replace("klffe_init_0", &format!("klffe_init_{index}"));
    let wrapper_path = compile_source(scratch, &format!("frontend-{index}.c"), &wrapper)?;
    let adapter_path = compile_source(scratch, &format!("adapters-{index}.ll"), &adapters)?;
    Ok((vec![wrapper_path, adapter_path], archive))
}

fn compile_kira(
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

fn entries(has_format: bool, has_compat: bool, has_run: bool, has_project: bool, has_emit: bool) -> Vec<(&'static str, usize)> {
    let mut entries = ENTRIES.to_vec();
    if has_format { entries.push(("klfFrontendFormat", 3)); }
    if has_compat { entries.push(("klfFrontendCompatDriver", 1)); }
    if has_run { entries.push(("klfFrontendRunProgram", 2)); }
    if has_project { entries.push(("klfFrontendProject", 2)); }
    if has_emit { entries.push(("klfFrontendEmit", 1)); }
    entries
}

fn validate_entries(module: &Module, has_format: bool, has_compat: bool, has_run: bool, has_project: bool, has_emit: bool) -> Result<(), String> {
    for (name, count) in entries(has_format, has_compat, has_run, has_project, has_emit) {
        let function = module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) if function.name == name => Some(function),
                _ => None,
            })
            .ok_or_else(|| format!("Kira frontend is missing RIDL entry `{name}`"))?;
        if !function.generics.is_empty()
            || function.params.len() != count
            || function.params.iter().any(|param| {
                param.convention != Convention::Owned || param.ty != Type::scalar(Scalar::I64)
            })
            || function.result != Some(Type::scalar(Scalar::I64))
        {
            return Err(format!(
                "Kira frontend entry `{name}` has the wrong RIDL signature"
            ));
        }
    }
    Ok(())
}

fn trampoline(metadata: &str, has_format: bool, has_compat: bool, has_run: bool, has_project: bool, has_emit: bool) -> String {
    let escaped = metadata
        .as_bytes()
        .iter()
        .map(|byte| format!("\\{byte:03o}"))
        .collect::<String>();
    let format_declaration = if has_format { "extern word KLFFrontend_klfFrontendFormat(word, word, word);" } else { "" };
    let format_case = if has_format { "case 7: return KLFFrontend_klfFrontendFormat(a, b, c);" } else { "" };
    let compat_declaration = if has_compat { "extern word KLFFrontend_klfFrontendCompatDriver(word);" } else { "" };
    let compat_case = if has_compat { "case 4: return KLFFrontend_klfFrontendCompatDriver(a);" } else { "" };
    let run_declaration = if has_run { "extern word KLFFrontend_klfFrontendRunProgram(word, word);" } else { "" };
    let run_case = if has_run { "case 5: return KLFFrontend_klfFrontendRunProgram(a, b);" } else { "" };
    let emit_declaration = if has_emit { "extern word KLFFrontend_klfFrontendEmit(word);" } else { "" };
    let emit_case = if has_emit { "case 6: return KLFFrontend_klfFrontendEmit(a);" } else { "" };
    let project_declaration = if has_project { "extern word KLFFrontend_klfFrontendProject(word, word);" } else { "" };
    let project_case = if has_project { "case 9: return KLFFrontend_klfFrontendProject(a, b);" } else { "" };
    format!(
        r#"typedef long long word;
extern word KLFFrontend_klfFrontendUnits(word, word);
extern word KLFFrontend_klfFrontendParse(word, word, word);
extern word KLFFrontend_klfFrontendElaborate(word, word, word);
extern word KLFFrontend_klfFrontendLower(word, word, word);
extern word KLFFrontend_klfFrontendBegin(void);
extern word KLFFrontend_klfFrontendDispose(void);
{format_declaration}
{compat_declaration}
{run_declaration}
{project_declaration}
{emit_declaration}
extern void klfrt_ridl_register(const void *, unsigned long long);
static void (*volatile klf_register_anchor)(const void *, unsigned long long) = klfrt_ridl_register;
const char *klffe_metadata_0(void) {{ return "{escaped}"; }}
#if !defined(_WIN32)
extern void __klf_init_KLFFrontend(void) __attribute__((weak));
void klffe_init_0(void) {{ (void)klf_register_anchor; if (__klf_init_KLFFrontend) __klf_init_KLFFrontend(); }}
#else
void klffe_init_0(void) {{ (void)klf_register_anchor; }}
#endif
word klffe_phase_0(word phase, word a, word b, word c) {{
    switch (phase) {{
    case 0: return KLFFrontend_klfFrontendUnits(a, b);
    case 1: return KLFFrontend_klfFrontendParse(a, b, c);
    case 2: return KLFFrontend_klfFrontendElaborate(a, b, c);
    case 3: return KLFFrontend_klfFrontendLower(a, b, c);
    {compat_case}
    {run_case}
    {format_case}
    {project_case}
    {emit_case}
    case 10: return KLFFrontend_klfFrontendDispose();
    case 11: return KLFFrontend_klfFrontendBegin();
    default: return -1;
    }}
}}
"#
    )
}

fn compile_source(scratch: &Scratch, name: &str, source: &str) -> Result<PathBuf, String> {
    let input = scratch.write(name, source.as_bytes())?;
    let output = input.with_extension("o");
    let clang = crate::find_clang().ok_or("no bundled clang for the Kira frontend")?;
    let result = Command::new(clang)
        .args(["-c", "-fexceptions", "-Wno-override-module"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|error| format!("cannot compile {}: {error}", input.display()))?;
    if !result.status.success() {
        return Err(format!(
            "cannot compile {}: {}",
            input.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(output)
}

fn read_sources(root: &Path) -> Result<DefinitionPackage, String> {
    fn gather(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                gather(&path, paths)?;
            } else if path
                .extension()
                .is_some_and(|extension| extension == "kira")
            {
                paths.push(path);
            }
        }
        Ok(())
    }
    let mut paths = vec![root.join("package.kira")];
    gather(&root.join("app"), &mut paths)?;
    paths.sort();
    let sources = paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            Ok(DefinitionSource::new(name, text))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(DefinitionPackage::from_sources(sources))
}

fn definition_hash(package: &DefinitionPackage) -> String {
    let mut bytes = Vec::new();
    for source in package.sources() {
        bytes.extend_from_slice(source.name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(source.text.as_bytes());
        bytes.push(0);
    }
    klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&bytes))
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "klf-kira-frontend-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        Ok(Self(path))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
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
mod tests {
    use super::*;

    const REQUIRED: &str = "\
function klfFrontendUnits(a: I64, b: I64) -> I64\n\
function klfFrontendParse(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendElaborate(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendLower(a: I64, b: I64, c: I64) -> I64\n\
function klfFrontendBegin() -> I64\n\
function klfFrontendDispose() -> I64\n";

    #[test]
    fn format_entry_is_required_only_when_the_hook_is_declared() {
        let parse = |text: &str| klf_ridl::parse(FileId::from_u32(0), "KLFFrontend", text).expect("RIDL");
        assert!(validate_entries(&parse(REQUIRED), false, false, false, false, false).is_ok());
        assert!(validate_entries(&parse(REQUIRED), true, false, false, false, false).unwrap_err().contains("klfFrontendFormat"));
        let wrong = format!("{REQUIRED}function klfFrontendFormat(a: I64) -> I64\n");
        assert!(validate_entries(&parse(&wrong), true, false, false, false, false).unwrap_err().contains("wrong RIDL signature"));
        let correct = format!("{REQUIRED}function klfFrontendFormat(a: I64, b: I64, c: I64) -> I64\n");
        assert!(validate_entries(&parse(&correct), true, false, false, false, false).is_ok());
        assert!(validate_entries(&parse(&correct), true, true, false, false, false).unwrap_err().contains("klfFrontendCompatDriver"));
        let both = format!("{correct}function klfFrontendCompatDriver(count: I64) -> I64\n");
        assert!(validate_entries(&parse(&both), true, true, false, false, false).is_ok());
        assert!(validate_entries(&parse(&both), true, true, true, false, false).unwrap_err().contains("klfFrontendRunProgram"));
        let all = format!("{both}function klfFrontendRunProgram(count: I64, total: I64) -> I64\n");
        assert!(validate_entries(&parse(&all), true, true, true, false, false).is_ok());
    }

    #[test]
    fn shader_emit_requires_a_matching_entry_and_dispatches_it() {
        let parse = |text: &str| klf_ridl::parse(FileId::from_u32(0), "KLFFrontend", text).expect("RIDL");
        assert!(validate_entries(&parse(REQUIRED), false, false, false, false, true)
            .unwrap_err().contains("klfFrontendEmit"));
        let contract = format!("{REQUIRED}function klfFrontendEmit(unit: I64) -> I64\n");
        assert!(validate_entries(&parse(&contract), false, false, false, false, true).is_ok());
        let source = trampoline("metadata", false, false, false, false, true);
        assert!(source.contains("case 6: return KLFFrontend_klfFrontendEmit(a);"));
    }

    #[test]
    fn package_module_after_kira_dependencies_overrides_construct_name() {
        let scratch = Scratch::new().unwrap();
        scratch.write("package.kira", br#"construct Original extends Package {
            let dependencies: [Dependency] = [Dependency { name: "KLF", path: "../KLF" }]
            let moduleRoot: String = "AnotherFrontend"
        }"#).unwrap();
        let mut session = Session::new("x86_64-pc-windows-msvc".parse().unwrap());
        assert_eq!(package_module(&mut session, &scratch.0).unwrap(), "AnotherFrontend");
    }
}
