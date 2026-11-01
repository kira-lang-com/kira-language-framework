//! RIDL declarations and native entry adapters for kernel services.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use klf_ridl::module::{Convention, FunctionDecl, Item, ItemKind, Module, Param};
use klf_ridl::types::{Scalar, Type};
use klf_services::{KERNEL_SERVICE_MANIFEST, KernelServiceManifest, ServiceType};

fn scalar(ty: ServiceType) -> Result<Scalar, String> {
    match ty {
        ServiceType::Integer {
            bits: 8,
            signed: true,
        } => Ok(Scalar::I8),
        ServiceType::Integer {
            bits: 16,
            signed: true,
        } => Ok(Scalar::I16),
        ServiceType::Integer {
            bits: 32,
            signed: true,
        } => Ok(Scalar::I32),
        ServiceType::Integer {
            bits: 64,
            signed: true,
        } => Ok(Scalar::I64),
        ServiceType::Integer {
            bits: 128,
            signed: true,
        } => Ok(Scalar::I128),
        ServiceType::Integer {
            bits: 8,
            signed: false,
        } => Ok(Scalar::U8),
        ServiceType::Integer {
            bits: 16,
            signed: false,
        } => Ok(Scalar::U16),
        ServiceType::Integer {
            bits: 32,
            signed: false,
        } => Ok(Scalar::U32),
        ServiceType::Integer {
            bits: 64,
            signed: false,
        } => Ok(Scalar::U64),
        ServiceType::Integer {
            bits: 128,
            signed: false,
        } => Ok(Scalar::U128),
        ServiceType::Float { bits: 32 } => Ok(Scalar::F32),
        ServiceType::Float { bits: 64 } => Ok(Scalar::F64),
        ServiceType::Pointer => Ok(Scalar::RawPtr),
        ServiceType::Void => Ok(Scalar::Void),
        other => Err(format!("unsupported kernel service type {other:?}")),
    }
}

/// Builds the RIDL contract from the kernel's service manifest.
///
/// # Errors
/// Returns an error if the manifest or one of its service types is invalid.
pub fn kernel_ridl_module() -> Result<Module, String> {
    let manifest =
        KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).map_err(|error| error.to_string())?;
    let mut module = Module::new("KLFKernel");
    for service in manifest.services() {
        let params = service
            .signature
            .parameters
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                let ty = scalar(*ty)?;
                if ty == Scalar::Void {
                    return Err(format!("{} has a void parameter", service.name));
                }
                Ok(Param {
                    label: format!("arg{index}"),
                    convention: Convention::Owned,
                    ty: Type::scalar(ty),
                    default: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let result = scalar(service.signature.result)?;
        module.items.push(Item {
            annotations: Vec::new(),
            kind: ItemKind::Function(FunctionDecl {
                name: service.symbol.clone(),
                generics: Vec::new(),
                self_param: None,
                params,
                result: (result != Scalar::Void).then_some(Type::scalar(result)),
            }),
        });
    }
    Ok(module)
}

/// Emits native RIDL entries that call the internal service implementations.
///
/// # Errors
/// Returns an error when the manifest or target's RIDL lowering is invalid.
pub fn kernel_ridl_entries(target: &str) -> Result<String, String> {
    if !matches!(
        target.split('-').next(),
        Some("x86_64" | "aarch64" | "wasm32")
    ) {
        return Err(format!("unsupported RIDL kernel target `{target}`"));
    }
    let module = kernel_ridl_module()?;
    let callees = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Function(function) => Some((function.name.clone(), function.name.clone())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    klf_ridl::llvm::entries_requiring_runtime(
        &module,
        &callees,
        &BTreeMap::new(),
        target,
        &str::to_owned,
    )
}

pub(crate) fn compile_entries(directory: &Path, target: &str) -> Result<PathBuf, String> {
    let clang = crate::kernel::find_clang().ok_or("no bundled clang for KLF RIDL entries")?;
    let source = directory.join("klf-kernel-ridl.ll");
    let object = directory.join("klf-kernel-ridl.o");
    std::fs::write(&source, kernel_ridl_entries(target)?)
        .map_err(|error| format!("cannot write {}: {error}", source.display()))?;
    let result = Command::new(clang)
        .args(["-c", "-x", "ir"])
        .arg(format!("--target={}", klf_target::machine_triple(target)))
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .map_err(|error| format!("cannot compile KLF RIDL entries: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "cannot compile KLF RIDL entries: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(object)
}

fn kira_target(triple: &str) -> String {
    let arch = triple.split('-').next().unwrap_or_default();
    let last = triple.rsplit('-').next().unwrap_or_default();
    if triple.contains("darwin") || triple.contains("macos") {
        return format!("{arch}-macos-none");
    }
    if triple.contains("ios") {
        let abi = if triple.ends_with("-sim") || triple.ends_with("-simulator") {
            "simulator"
        } else {
            "none"
        };
        return format!("{arch}-ios-{abi}");
    }
    if triple.contains("windows") {
        return format!("{arch}-windows-{last}");
    }
    if triple.contains("emscripten") {
        return format!("{arch}-emscripten-unknown");
    }
    if klf_target::is_tessera(triple) {
        return triple.to_owned();
    }
    if triple.contains("linux") {
        let abi = if last == "linux" { "gnu" } else { last };
        return format!("{arch}-linux-{abi}");
    }
    triple.to_owned()
}

pub(crate) fn write_kira_package(out: &Path, targets: &[String], module: &Module) -> Result<(), String> {
    let root = out.join("share/klf/KLF");
    let sources = root.join("app");
    std::fs::create_dir_all(&sources)
        .map_err(|error| format!("cannot create {}: {error}", sources.display()))?;
    let mut rows = BTreeMap::new();
    for target in targets {
        let key = kira_target(target);
        if let Some(previous) = rows.insert(key.clone(), target) {
            return Err(format!("Kira target {key} is shared by {previous} and {target}"));
        }
    }
    let entries = rows.iter().map(|(triple, target)| format!(
        "            NativeTarget {{ triple: \"{triple}\", staticLib: \"../../../platforms/{target}/libklf_kernel_ridl.a\" }},\n"
    )).collect::<String>();
    let manifest = format!(
        "construct KLF extends Package {{\n    let kind: PackageKind = .Library\n    let moduleRoot: String = \"KLF\"\n    let nativeLibraries: [NativeLibrary] = [\n        NativeLibrary {{ name: \"KLFKernel\", nativeTargets: [\n{entries}        ] }}\n    ]\n}}\n"
    );
    std::fs::write(root.join("package.kira"), manifest)
        .map_err(|error| format!("cannot write KLF package: {error}"))?;
    std::fs::write(root.join("README.md"), include_str!("../../../library/KLF/README.md"))
        .map_err(|error| format!("cannot write KLF package documentation: {error}"))?;
    let bindings = klf_ridl::bindgen::kira_bindings(module).replace(
        "@Ridl(\"KLFKernel\") function ",
        "@Ridl(\"KLFKernel\") public function ",
    );
    std::fs::write(sources.join("Kernel.kira"), bindings)
        .map_err(|error| format!("cannot write KLF bindings: {error}"))?;
    let typed = crate::kcore_bindings::generate()?;
    std::fs::write(sources.join("KCore.kira"), typed)
        .map_err(|error| format!("cannot write KCore service bindings: {error}"))?;
    std::fs::write(
        sources.join("KCoreText.kira"),
        include_str!("../../../library/KLF/KCoreText.kira"),
    )
    .map_err(|error| format!("cannot write KCore text helpers: {error}"))?;
    std::fs::write(
        sources.join("Language.kira"),
        include_str!("../../../library/KLF/Language.kira"),
    )
    .map_err(|error| format!("cannot write Language interface: {error}"))?;
    std::fs::write(
        sources.join("ShaderLanguage.kira"),
        include_str!("../../../library/KLF/ShaderLanguage.kira"),
    )
    .map_err(|error| format!("cannot write ShaderLanguage interface: {error}"))?;
    std::fs::write(sources.join("Editor.kira"), include_str!("../../../library/KLF/Editor.kira"))
        .map_err(|error| format!("cannot write editor interface: {error}"))?;
    std::fs::write(sources.join("Layout.kira"), include_str!("../../../library/KLF/Layout.kira"))
        .map_err(|error| format!("cannot write formatting layout: {error}"))?;
    std::fs::write(
        sources.join("Session.kira"),
        include_str!("../../../library/KLF/Session.kira"),
    )
    .map_err(|error| format!("cannot write Session interface: {error}"))?;
    std::fs::write(
        sources.join("Driver.kira"),
        include_str!("../../../library/KLF/Driver.kira"),
    )
    .map_err(|error| format!("cannot write driver helpers: {error}"))?;
    std::fs::write(
        sources.join("Artifacts.kira"),
        include_str!("../../../library/KLF/Artifacts.kira"),
    )
    .map_err(|error| format!("cannot write artifact interface: {error}"))?;
    std::fs::write(
        sources.join("TestSupport.kira"),
        include_str!("../../../library/KLF/TestSupport.kira"),
    )
    .map_err(|error| format!("cannot write frontend test support: {error}"))?;
    let tests = root.join("tests");
    std::fs::create_dir_all(&tests)
        .map_err(|error| format!("cannot create {}: {error}", tests.display()))?;
    std::fs::write(
        tests.join("KCore.kira"),
        include_str!("../../../library/KLF/tests/KCore.kira"),
    )
    .map_err(|error| format!("cannot write KLF tests: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_selects_archives_by_kira_target() {
        let directory = std::env::temp_dir().join(format!("klf-kira-targets-{}", std::process::id()));
        let targets = vec!["x86_64-pc-windows-msvc".to_owned(), "aarch64-unknown-linux-gnu".to_owned()];
        write_kira_package(&directory, &targets, &kernel_ridl_module().unwrap()).unwrap();
        let manifest = std::fs::read_to_string(directory.join("share/klf/KLF/package.kira")).unwrap();
        assert!(manifest.contains("triple: \"x86_64-windows-msvc\", staticLib: \"../../../platforms/x86_64-pc-windows-msvc/libklf_kernel_ridl.a\""));
        assert!(manifest.contains("triple: \"aarch64-linux-gnu\", staticLib: \"../../../platforms/aarch64-unknown-linux-gnu/libklf_kernel_ridl.a\""));
        assert_eq!(kira_target("aarch64-apple-darwin"), "aarch64-macos-none");
        assert_eq!(kira_target("x86_64-unknown-tessera"), "x86_64-unknown-tessera");
        let ambiguous = vec!["x86_64-linux-gnu".to_owned(), "x86_64-unknown-linux-gnu".to_owned()];
        assert!(write_kira_package(&directory, &ambiguous, &kernel_ridl_module().unwrap()).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn kernel_entry_object_uses_the_requested_target() {
        let directory = std::env::temp_dir().join(format!("klf-kernel-cross-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let object = compile_entries(&directory, "aarch64-unknown-linux-gnu").unwrap();
        let bytes = std::fs::read(object).unwrap();
        assert_eq!(&bytes[..4], b"\x7fELF");
        assert_eq!(&bytes[18..20], &183_u16.to_le_bytes());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
