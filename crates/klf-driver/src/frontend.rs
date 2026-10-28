//! Building a definition package into a native frontend binary.
//!
//! Section 5.2 asks `klf build` on a definition package for the tool that
//! package describes: a compiler driver for the defined language, named by the
//! `Language`'s own `driver` metadata. What comes out is the definition's phase
//! code compiled by Cranelift, the entry wrapper from
//! [`klf_cranelift::emit_frontend_object`], and `klf-frontend-rt`, which holds
//! the service implementations and the driver loop. The binary runs the same
//! `klf_defsvc::drive` schedule over the same `klf_defsvc` service bridge the
//! VM-hosted path uses; only the phase calls differ.

use crate::kernel::{KernelArchive, host_triple, kernel_archive};
use crate::language::{PackageError, load_package};
use crate::native::host_module;
use crate::{Backend, LanguageDefinition};
use klf_defsvc::{FrontendMetadata, Hook, phase_materializes_artifact};
use klf_diag::progress;
use klf_ridl::sha256::{hex, sha256};
use klf_services::{DefinitionPackage, DefinitionPhase, Session};
use klf_target::{Target, cache_root};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// What `klf build --profile` asks for (section 5.2). A release build runs
/// the KIR optimizer of section 14.1 over what it compiles; a debug build runs
/// only the mandatory passes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Profile {
    #[default]
    Debug,
    Release,
}

impl Profile {
    /// The profile `--profile` names: `debug` or `release`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "debug" => Some(Self::Debug),
            "release" => Some(Self::Release),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    /// The tier-1 level of section 14.1 the profile runs.
    #[must_use]
    pub const fn level(self) -> klf_kir_passes::Level {
        match self {
            Self::Debug => klf_kir_passes::Level::O0,
            Self::Release => klf_kir_passes::Level::O2,
        }
    }
}

/// What a native frontend build produced.
pub struct BuiltFrontend {
    /// The linked executable.
    pub binary: PathBuf,
    /// The driver name the definition declared, which is what the binary is
    /// named when no `-o` was given.
    pub driver: String,
}

/// Reads the package at `root` — a directory or a single `.klfd` file — and
/// links a native frontend for it at `output`. A shader definition's binary
/// also holds the other shader languages named in `languages`, each compiled
/// once, which is how its `emit` phase reaches an out
/// (`KLF-KSL.md` section 5.9).
///
/// # Errors
/// Returns a rendered message when a package cannot be read or compiled,
/// when the native toolchain is missing, or when the link fails.
pub fn build_frontend(
    session: &mut Session,
    root: &Path,
    output: Option<&Path>,
    target: &Target,
    languages: &[&Path],
    profile: Profile,
) -> Result<BuiltFrontend, String> {
    progress!("read definition package");
    let package = read_package(root)?;
    let built = compile(session, root, &package, 0, target, profile)?;
    let mut objects = vec![built.object];
    if built.shader {
        let own = root.canonicalize().ok();
        for language in languages {
            let language: &Path = language;
            if language.canonicalize().ok() == own {
                return Err(format!(
                    "`{}` names the package being built; a frontend holds one definition once",
                    language.display()
                ));
            }
            let package = read_package(language)?;
            let object = cached(&package, objects.len(), target, profile, |index| {
                compile(session, language, &package, index, target, profile)
                    .map(|built| built.object)
            })?;
            objects.push(object);
        }
    }
    objects.push(
        klf_cranelift::emit_frontend_registry(host_triple(), objects.len())
            .map_err(|error| format!("cranelift: {error}"))?,
    );
    let binary = output.map_or_else(|| PathBuf::from(&built.driver), Path::to_path_buf);
    progress!("link {}", built.driver);
    link(&objects, &binary, &built.driver)?;
    Ok(BuiltFrontend {
        binary,
        driver: built.driver,
    })
}

/// One definition compiled into its object of a frontend binary.
struct Compiled {
    object: Vec<u8>,
    driver: String,
    shader: bool,
}

fn compile(
    session: &mut Session,
    root: &Path,
    package: &DefinitionPackage,
    index: usize,
    target: &Target,
    profile: Profile,
) -> Result<Compiled, String> {
    let definition = LanguageDefinition::from_package(session, package, Backend::Vm)
        .ok_or_else(|| format!("cannot compile definition package {}", root.display()))?;

    let descriptor = definition.descriptor();
    let mut metadata = FrontendMetadata::from_descriptor(descriptor, target.triple());
    metadata.definition = definition_hash(package);
    let entrypoints = &definition.metadata().entrypoints;
    for (hook, declared) in [
        (Hook::CompatDriver, entrypoints.compat_driver.is_some()),
        (Hook::RunProgram, entrypoints.run_program.is_some()),
        (Hook::Emit, entrypoints.emit.is_some()),
        (Hook::Format, entrypoints.format.is_some()),
        (Hook::Project, entrypoints.project.is_some()),
    ] {
        if declared {
            metadata.declare(hook);
        }
    }
    if descriptor.shader.is_none() {
        klf_defsvc::driver_cli::validate(&metadata)
            .map_err(|error| format!("{}: command line: {error}", root.display()))?;
    }
    let library = descriptor
        .shader
        .as_ref()
        .map_or(descriptor.std_root.as_str(), |shader| {
            shader.library_root.as_str()
        });
    if !library.is_empty() {
        let std_root = root.join(library);
        let std_root = std_root.canonicalize().unwrap_or(std_root);
        metadata.std_root = std_root.to_string_lossy().into_owned();
    }
    for phase in [
        DefinitionPhase::Units,
        DefinitionPhase::Parse,
        DefinitionPhase::Elaborate,
        DefinitionPhase::Lower,
    ] {
        metadata.materializes[phase.index()] =
            phase_materializes_artifact(definition.image(), phase);
    }

    let mut module = host_module(definition.image().module());
    if profile == Profile::Release {
        progress!("optimize {}", descriptor.name);
        klf_kir_passes::optimize(
            &mut module,
            profile.level(),
            klf_kir_passes::Backend::Cranelift,
        );
    }
    // The KIR the frontend's machine code comes from, for reading what a
    // definition lowers to.
    if index == 0
        && let Some(path) = std::env::var_os("KLF_DUMP_FRONTEND_KIR")
    {
        std::fs::write(path, module.to_string())
            .map_err(|error| format!("frontend KIR dump: {error}"))?;
    }
    let object = klf_cranelift::emit_frontend_object(&module, &metadata.encode(), index)
        .map_err(|error| format!("cranelift: {error}"))?;
    progress!("codegen {}", descriptor.name);

    let driver = if descriptor.driver.is_empty() {
        descriptor.name.clone()
    } else {
        descriptor.driver.clone()
    };
    Ok(Compiled {
        object,
        driver,
        shader: descriptor.shader.is_some(),
    })
}

/// The object of a bundled definition, from the KLF cache when this `klf`
/// compiled the same sources at the same index for the same target before.
/// A third-party shader definition's build then compiles only itself.
fn cached(
    package: &DefinitionPackage,
    index: usize,
    target: &Target,
    profile: Profile,
    compile: impl FnOnce(usize) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let Some(root) = cache_root() else {
        return compile(index);
    };
    let mut key = format!(
        "{}\n{}\n{}\n{index}\n",
        env!("CARGO_PKG_VERSION"),
        target.triple(),
        profile.name()
    );
    // A development `klf` keeps its release number across changes to the
    // code that compiles a definition, so its own build is part of the key.
    if let Some(built) = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.metadata().ok())
    {
        let modified = built
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        let _ = writeln!(key, "{modified} {}", built.len());
    }
    for source in package.sources() {
        let _ = writeln!(
            key,
            "{} {}",
            source.name,
            hex(&sha256(source.text.as_bytes()))
        );
    }
    let path = root
        .join("frontends")
        .join(format!("{}.o", hex(&sha256(key.as_bytes()))));
    if let Ok(object) = std::fs::read(&path) {
        return Ok(object);
    }
    let object = compile(index)?;
    // A cache that cannot be written only costs the next build its time.
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        let _ = std::fs::write(&path, &object);
    }
    Ok(object)
}

/// SHA-256 over every source of `package`, by name and text, in hex.
fn definition_hash(package: &DefinitionPackage) -> String {
    let mut hashed = String::new();
    for source in package.sources() {
        let _ = writeln!(
            hashed,
            "{} {}",
            source.name,
            hex(&sha256(source.text.as_bytes()))
        );
    }
    hex(&sha256(hashed.as_bytes()))
}

fn read_package(root: &Path) -> Result<DefinitionPackage, String> {
    if root.is_dir() {
        return load_package(root).map_err(|error: PackageError| error.to_string());
    }
    let text = std::fs::read_to_string(root)
        .map_err(|error| format!("cannot read {}: {error}", root.display()))?;
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    Ok(DefinitionPackage::single(name, text))
}

fn link(objects: &[Vec<u8>], binary: &Path, driver: &str) -> Result<(), String> {
    // Builds of one driver may run at once, as tests do, so each links from
    // its own directory.
    static LINKS: AtomicUsize = AtomicUsize::new(0);
    let runtime = kernel_archive(KernelArchive::Driver, host_triple())?;

    let directory = std::env::temp_dir().join(format!(
        "klf-frontend-{driver}-{}-{}",
        std::process::id(),
        LINKS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        let path = directory.join(format!("{driver}-{index}.o"));
        std::fs::write(&path, object).map_err(|error| error.to_string())?;
        paths.push(path);
    }

    let linked = crate::kernel::link(&paths, &[runtime], binary, host_triple());
    let _ = std::fs::remove_dir_all(&directory);
    linked?;
    Ok(())
}
