//! `klf build <defdir> --out <dir>`: steps 3 to 5 of the section 5.3 pipeline.
//!
//! The build links the definition's native driver, compiles the language's std
//! with that driver as a subprocess, and assembles the sysroot of section 5.4.
//! Everything is written to a staging directory first; if any step fails the
//! staging directory is removed and `<out>` is left as it was.

use crate::kernel::{KernelArchive, host_triple, kernel_archive};
use crate::{Backend, LanguageDefinition, Profile, build_frontend};
use klf_diag::progress;
use klf_services::{DefinitionPackage, Session};
use klf_target::Target;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

struct StdSources {
    /// The packages the language's own unit compiles: Foundation and the
    /// packages its manifest reaches by path.
    driver: Vec<PathBuf>,
    /// The other std packages, the bundled libraries a program imports by
    /// name. They are installed beside the std and built by each program that
    /// imports them, never inside the std unit.
    bundled: Vec<PathBuf>,
    /// Everything under `<stdRoot>/include`: the headers programs include,
    /// installed as `include/<lang>/` rather than compiled.
    headers: Vec<PathBuf>,
}

impl StdSources {
    fn is_empty(&self) -> bool {
        self.driver.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = &PathBuf> {
        self.driver
            .iter()
            .chain(self.bundled.iter())
            .chain(self.headers.iter())
    }
}

/// What a sysroot build produced.
pub struct BuiltSysroot {
    pub root: PathBuf,
    pub driver: PathBuf,
    /// `bin/<driver>-run`, the bytecode runner; empty for a shader
    /// language, whose programs do not run on the host.
    pub runner: PathBuf,
    /// The std static library for each target, when the package has std
    /// sources.
    pub std_libraries: Vec<PathBuf>,
}

/// Builds the toolchain for the definition package at `package` into `out`. A
/// shader package's driver also holds the other shader languages named in
/// `languages`, which is how its `emit` phase reaches an out.
///
/// # Errors
/// Returns a rendered message when the definition does not compile, the std
/// does not compile with the generated driver, or a file cannot be written.
pub fn build_sysroot(
    session: &mut Session,
    package: &Path,
    out: &Path,
    targets: &[Target],
    languages: &[&Path],
    profile: Profile,
) -> Result<BuiltSysroot, String> {
    if targets.is_empty() {
        return Err("a sysroot needs at least one target".into());
    }
    if out.exists() && !replaceable(out) {
        return Err(format!(
            "{} exists and is not a sysroot (no definition.lock); refusing to replace it",
            out.display()
        ));
    }
    let loaded = crate::language::load_package(package).map_err(|error| error.to_string())?;
    let definition = LanguageDefinition::from_package(session, &loaded, Backend::Vm)
        .ok_or_else(|| format!("cannot compile definition package {}", package.display()))?;
    let descriptor = definition.descriptor().clone();

    let staging = staging_directory(out)?;
    let result = if descriptor.shader.is_some() {
        crate::shader_sysroot::assemble(
            session,
            package,
            &loaded,
            &descriptor,
            &staging,
            languages,
            profile,
        )
    } else {
        assemble(
            session,
            package,
            &loaded,
            &descriptor,
            &staging,
            targets,
            profile,
        )
    };
    let built = match result {
        Ok(built) => built,
        Err(error) => {
            // `KLF_KEEP_STAGING` keeps a failed build's staging directory, so
            // its driver can be run on the inputs that broke the std.
            if std::env::var_os("KLF_KEEP_STAGING").is_none() {
                let _ = std::fs::remove_dir_all(&staging);
            }
            return Err(error);
        }
    };
    if out.exists() {
        std::fs::remove_dir_all(out)
            .map_err(|error| format!("cannot replace {}: {error}", out.display()))?;
    }
    std::fs::rename(&staging, out)
        .map_err(|error| format!("cannot move the sysroot to {}: {error}", out.display()))?;
    let relocate = |path: PathBuf| match path.strip_prefix(&staging) {
        Ok(relative) => out.join(relative),
        Err(_) => path,
    };
    Ok(BuiltSysroot {
        root: out.to_path_buf(),
        driver: relocate(built.driver),
        runner: relocate(built.runner),
        std_libraries: built.std_libraries.into_iter().map(relocate).collect(),
    })
}

fn replaceable(out: &Path) -> bool {
    out.join("definition.lock").exists()
        || std::fs::read_dir(out).is_ok_and(|mut entries| entries.next().is_none())
}

fn staging_directory(out: &Path) -> Result<PathBuf, String> {
    let name = out
        .file_name()
        .ok_or_else(|| format!("{} has no directory name", out.display()))?;
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = parent.join(format!(".{}.klf-staging", name.to_string_lossy()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    create(&staging)?;
    Ok(staging)
}

pub(crate) fn create(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))
}

fn assemble(
    session: &mut Session,
    package: &Path,
    loaded: &DefinitionPackage,
    descriptor: &klf_services::LanguageDescriptor,
    staging: &Path,
    targets: &[Target],
    profile: Profile,
) -> Result<BuiltSysroot, String> {
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
    progress!("build driver {}", driver_name);
    build_frontend(session, package, Some(&driver), &host_target, &[], profile)?;

    // Canonical, as `std_sources` reads it: the sources then strip this prefix
    // exactly, so each one is copied to its own place under the sysroot.
    let std_root = std::fs::canonicalize(package.join(&descriptor.std_root))
        .unwrap_or_else(|_| package.join(&descriptor.std_root));
    let std_sources = std_sources(&std_root, &descriptor.extensions)?;
    install_headers(
        &std_root,
        &std_sources.headers,
        &staging.join("include").join(&descriptor.name),
    )?;
    install_std_sources(
        &std_root,
        &std_sources,
        &staging.join("share").join(&descriptor.name).join("std"),
    )?;
    let language = staging.join("lib").join(&descriptor.name);
    let mut std_libraries = Vec::new();
    let mut widths = Vec::new();
    for target in targets {
        let triple = target.triple();
        let runtime_directory = staging.join("lib/klf").join(triple);
        create(&runtime_directory)?;
        for archive in [KernelArchive::Runtime, KernelArchive::Vm] {
            let built = kernel_archive(archive, triple)?;
            std::fs::copy(&built, runtime_directory.join(archive.file_name()))
                .map_err(|error| format!("cannot copy {}: {error}", built.display()))?;
        }
        if std_sources.is_empty() {
            continue;
        }
        let native = language.join(triple);
        create(&native)?;
        progress!("build std for {triple}");
        let library = native.join("libstd.a");
        run_std(&driver, &std_sources, triple, &library)?;
        let shared = native.join(if triple.contains("-apple-") {
            "libstd.dylib"
        } else {
            "libstd.so"
        });
        run_driver(&driver, &std_sources.driver, triple, "shared", &shared)?;
        std_libraries.push(library);
        let width = target.pointer_width();
        if !widths.contains(&width) {
            widths.push(width);
            let bytecode = language.join("bytecode").join(width.to_string());
            create(&bytecode)?;
            if !std_sources.driver.is_empty() {
                run_driver(
                    &driver,
                    &std_sources.driver,
                    triple,
                    "klbc",
                    &bytecode.join("std.klbc"),
                )?;
            }
        }
    }

    if !std_sources.is_empty() {
        let share = staging.join("share").join(&descriptor.name);
        install_surface(
            &driver,
            &std_root,
            &std_sources,
            &share,
            &descriptor.name,
            targets[0].triple(),
        )?;
    }

    let host_std = std_libraries
        .iter()
        .find(|library| {
            library
                .parent()
                .is_some_and(|parent| parent.ends_with(host))
        })
        .cloned();
    let runner = staging.join("bin").join(format!("{driver_name}-run"));
    link_runner(&runner, host_std.as_deref(), descriptor.entry)?;

    let triples: Vec<&str> = targets.iter().map(Target::triple).collect();
    let lock = crate::lock::definition_lock(loaded, &std_root, std_sources.all(), "std", &triples)?;
    std::fs::write(staging.join("definition.lock"), lock)
        .map_err(|error| format!("cannot write definition.lock: {error}"))?;
    Ok(BuiltSysroot {
        root: staging.to_path_buf(),
        driver,
        runner,
        std_libraries,
    })
}

/// The std's frozen ABI records (section 24.4) travel with its sources in
/// `<stdRoot>/abi/`; the sysroot carries them in `share/<lang>/abi/`.
fn copy_abi_records(from: &Path, to: &Path) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    create(to)?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "ridlsig")
        {
            let name = path.file_name().unwrap_or_default();
            std::fs::copy(&path, to.join(name))
                .map_err(|error| format!("cannot copy {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// Puts `<driver>-run` in place by copying the runner `klf dist` built for
/// this triple. Nothing is compiled and nothing is linked: the runner is an
/// artifact of the distribution, and a language's entry convention is read
/// from `KLF_VM_ENTRY_CONVENTION` when it starts, so one file serves every
/// sysroot and no toolchain needs a C compiler to make one.
fn link_runner(
    runner: &Path,
    std_library: Option<&Path>,
    convention: klf_services::EntryConvention,
) -> Result<(), String> {
    let _ = std_library;
    let source = crate::kernel::runner(host_triple())?;
    std::fs::create_dir_all(runner.parent().unwrap_or(Path::new(".")))
        .map_err(|error| format!("cannot create the runner's directory: {error}"))?;
    std::fs::copy(&source, runner)
        .map(|_| ())
        .map_err(|error| format!("cannot install the runner {}: {error}", runner.display()))?;
    // One runner serves every language, so which entry point a program's
    // `main` is has to be said somewhere. It is a line beside the runner
    // rather than a second binary or a line compiled into it.
    let name = match convention {
        klf_services::EntryConvention::Outcome => "outcome",
        klf_services::EntryConvention::Process => "process",
        klf_services::EntryConvention::ProcessExit => "process-exit",
    };
    let path = runner.with_extension("convention");
    std::fs::write(&path, format!("{name}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Every file under `root` whose extension the language claims, sorted so the
/// std compiles in the same order on every build.
/// The std's RIDL surface, as the driver writes it for `triple`, and its ABI
/// records, under `share`.
fn install_surface(
    driver: &Path,
    std_root: &Path,
    std_sources: &StdSources,
    share: &Path,
    name: &str,
    triple: &str,
) -> Result<(), String> {
    let ridl = share.join("ridl");
    create(&ridl)?;
    if !std_sources.driver.is_empty() {
        run_driver(
            driver,
            &std_sources.driver,
            triple,
            "ridl",
            &ridl.join(format!("{name}.ridl")),
        )?;
    }
    copy_abi_records(&std_root.join("abi"), &share.join("abi"))
}

fn std_sources(root: &Path, extensions: &[String]) -> Result<StdSources, String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut sources = Vec::new();
    let include = root.join("include");
    let mut headers = Vec::new();
    if root.is_dir() {
        collect(&root, extensions, &mut sources)?;
        sources.retain(|source| !source.starts_with(&include));
    }
    if include.is_dir() {
        collect_all(&include, &mut headers)?;
    }
    let unit = unit_packages(&root)?;
    let mut driver = Vec::new();
    let mut bundled = Vec::new();
    for source in sources {
        match unit.as_ref() {
            Some(packages)
                if owning_package(&root, &source)
                    .is_some_and(|package| !packages.contains(&package)) =>
            {
                bundled.push(source)
            }
            _ => driver.push(source),
        }
    }
    driver.sort();
    bundled.sort();
    headers.sort();
    Ok(StdSources {
        driver,
        bundled,
        headers,
    })
}

/// The package directories the std unit compiles: `Foundation` and every
/// package its manifests reach by a path dependency. None when the std has no
/// `Foundation` package, in which case the whole std is the unit.
fn unit_packages(root: &Path) -> Result<Option<Vec<PathBuf>>, String> {
    let foundation = root.join("Foundation");
    if !foundation.join("package.kira").is_file() {
        return Ok(None);
    }
    let mut packages = vec![foundation];
    let mut next = 0;
    while next < packages.len() {
        let manifest = packages[next].join("package.kira");
        let text = std::fs::read_to_string(&manifest)
            .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
        for relative in manifest_paths(&text) {
            let dependency = packages[next].join(relative);
            let dependency = std::fs::canonicalize(&dependency)
                .map_err(|error| format!("cannot read {}: {error}", dependency.display()))?;
            if !packages.contains(&dependency) {
                packages.push(dependency);
            }
        }
        next += 1;
    }
    Ok(Some(packages))
}

/// The string literals after each `path:` in a manifest.
fn manifest_paths(text: &str) -> Vec<&str> {
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("path:") {
        let after = rest[at + "path:".len()..].trim_start();
        if let Some(quoted) = after.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                paths.push(&quoted[..end]);
            }
        }
        rest = &rest[at + "path:".len()..];
    }
    paths
}

/// The nearest directory under `root` that holds a `package.kira` above
/// `source`.
fn owning_package(root: &Path, source: &Path) -> Option<PathBuf> {
    let mut directory = source.parent()?;
    while directory != root && directory.starts_with(root) {
        if directory.join("package.kira").is_file() {
            return Some(directory.to_path_buf());
        }
        directory = directory.parent()?;
    }
    None
}

pub(crate) fn collect_all(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_all(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Copies the std headers into `include/<lang>/`, keeping their layout under
/// `<stdRoot>/include`, where an installed driver looks them up.
fn install_headers(std_root: &Path, headers: &[PathBuf], to: &Path) -> Result<(), String> {
    let include = std_root.join("include");
    for header in headers {
        let target = to.join(header.strip_prefix(&include).unwrap_or(header));
        if let Some(parent) = target.parent() {
            create(parent)?;
        }
        std::fs::copy(header, &target)
            .map_err(|error| format!("cannot copy {}: {error}", header.display()))?;
    }
    Ok(())
}

/// The std's sources, as `share/<lang>/std/`, where an installed driver
/// finds the packages its language imports by name (D-327).
fn install_std_sources(std_root: &Path, sources: &StdSources, to: &Path) -> Result<(), String> {
    for source in sources.all() {
        // A source outside `std_root` has no place under `to`. Joining it would
        // yield the source itself, and copying a file onto itself truncates it.
        let relative = source.strip_prefix(std_root).map_err(|_| {
            format!("std source {} is outside {}", source.display(), std_root.display())
        })?;
        let target = to.join(relative);
        if target == *source {
            return Err(format!("std source {} would be copied onto itself", source.display()));
        }
        if let Some(parent) = target.parent() {
            create(parent)?;
        }
        std::fs::copy(source, &target)
            .map_err(|error| format!("cannot copy {}: {error}", source.display()))?;
    }
    Ok(())
}

fn collect(directory: &Path, extensions: &[String], out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect(&path, extensions, out)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.iter().any(|claimed| claimed == extension))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn run_driver(
    driver: &Path,
    sources: &[PathBuf],
    target: &str,
    emit: &str,
    output: &Path,
) -> Result<(), String> {
    let result = Command::new(driver)
        .args(sources)
        .args(["--target", target, "--emit", emit, "-o"])
        .arg(output)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", driver.display()))?;
    if result.status.success() {
        return Ok(());
    }
    Err(format!(
        "compiling the std for `{target}` (--emit {emit}) failed: {}\n{}{}",
        describe_status(result.status),
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    ))
}

/// How a child ended, for a failure that printed nothing: an exit status, or
/// the signal that killed it.
fn describe_status(status: std::process::ExitStatus) -> String {
    match (status.code(), killing_signal(&status)) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => String::from("it ended without a status"),
    }
}

/// The signal that ended a child. Only Unix ends a process by signal; on
/// Windows every ending carries an exit code.
#[cfg(unix)]
fn killing_signal(status: &std::process::ExitStatus) -> Option<i32> {
    status.signal()
}

#[cfg(not(unix))]
fn killing_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// A language's std, as the language itself compiles it. There is no
/// `native.list`: no language hands the framework C or C++ sources, so every
/// std on every target comes out of the same `klf build <defdir> --emit lib`
/// the driver's own code uses.
fn run_std(driver: &Path, sources: &StdSources, target: &str, output: &Path) -> Result<(), String> {
    if sources.driver.is_empty() {
        return Ok(());
    }
    run_driver(driver, &sources.driver, target, "lib", output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/klf-sysroot-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().expect("a fixture file has a directory"))
                .expect("create fixture directory");
            std::fs::write(&file, text).expect("write fixture file");
        }
        std::fs::canonicalize(&root).expect("canonical fixture root")
    }

    fn relative(root: &Path, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .expect("under root")
                    .display()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn unit_is_foundation_and_its_path_closure_and_libraries_are_bundled() {
        let root = fixture(
            "unit-closure",
            &[
                (
                    "Foundation/package.kira",
                    "construct Foundation extends Package {\n    let dependencies: [Dependency] = [\n        Dependency { name: \"Heap\", path: \"../Heap\" }\n    ]\n}\n",
                ),
                ("Foundation/app/A.kira", "function a() {}\n"),
                ("Heap/package.kira", "construct Heap extends Package {}\n"),
                ("Heap/app/H.kira", "function h() {}\n"),
                (
                    "Build/package.kira",
                    "construct Build extends Package {\n    let dependencies: [Dependency] = [\n        Dependency { name: \"Kten\", path: \"../Kten\" }\n    ]\n}\n",
                ),
                ("Build/app/B.kira", "function b() {}\n"),
                ("Kten/package.kira", "construct Kten extends Package {}\n"),
                ("Kten/app/K.kira", "function k() {}\n"),
            ],
        );
        let sources = std_sources(&root, &["kira".to_owned()]).expect("std sources");
        assert_eq!(
            relative(&root, &sources.driver),
            [
                "Foundation/app/A.kira",
                "Foundation/package.kira",
                "Heap/app/H.kira",
                "Heap/package.kira"
            ]
        );
        assert_eq!(
            relative(&root, &sources.bundled),
            [
                "Build/app/B.kira",
                "Build/package.kira",
                "Kten/app/K.kira",
                "Kten/package.kira"
            ]
        );
        assert_eq!(sources.all().count(), 8);
    }

    #[test]
    fn std_without_foundation_is_one_unit() {
        let root = fixture(
            "no-foundation",
            &[
                ("Solo/package.kira", "construct Solo extends Package {}\n"),
                ("Solo/app/S.kira", "function s() {}\n"),
            ],
        );
        let sources = std_sources(&root, &["kira".to_owned()]).expect("std sources");
        assert_eq!(
            relative(&root, &sources.driver),
            ["Solo/app/S.kira", "Solo/package.kira"]
        );
        assert!(sources.bundled.is_empty());
    }
}
