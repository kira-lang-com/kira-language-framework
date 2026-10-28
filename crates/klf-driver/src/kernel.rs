//! Where the prebuilt kernel libraries and the bundled LLVM come from
//! (section 5.1).
//!
//! A `klf` running from a distribution (`<home>/bin/klf` next to
//! `<home>/kernel/services.manifest`, or `$KLF_HOME`) takes every archive from
//! `<home>/kernel/<target>/` and never runs Cargo: generating a toolchain
//! must not require a Rust installation. A `klf` running from the workspace
//! builds the archives with Cargo, which is how a distribution is made.

use crate::native::workspace_root;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A prebuilt kernel library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelArchive {
    /// The archive a generated driver links, and only the host ever builds
    /// one, so it sits beside `klf` in `bin` rather than in a platform.
    Driver,
    /// `libklfrt.a`: the runtime every native program links.
    Runtime,
    /// `libklfvm.a`: the VM core, with its C entry points.
    Vm,
}

impl KernelArchive {
    /// The file name in a distribution and a sysroot.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Driver => "libklf_kernel.a",
            Self::Runtime => "libklfrt.a",
            Self::Vm => "libklfvm.a",
        }
    }

    /// Whether the archive is per-target, and so lives in
    /// `platforms/<triple>/`. A driver's is not: it is linked into a compiler
    /// that runs on the host, and no other target's program needs it.
    #[must_use]
    pub const fn per_target(self) -> bool {
        match self {
            Self::Driver => false,
            Self::Runtime | Self::Vm => true,
        }
    }

    /// Where `target` keeps `self` in a distribution.
    #[must_use]
    pub fn distribution_path(self, home: &Path, target: &str) -> PathBuf {
        if self.per_target() {
            home.join("platforms").join(target).join(self.file_name())
        } else {
            home.join("bin").join(self.file_name())
        }
    }

    const fn package(self) -> &'static str {
        match self {
            Self::Driver => "klf-frontend-rt",
            Self::Runtime => "klf-native-rt",
            Self::Vm => "klf-vm-rt",
        }
    }

    /// Whether programs link the archive, which then goes without its debug
    /// information (see [`klf_target::without_debug_info`]).
    const fn linked_by_programs(self) -> bool {
        match self {
            Self::Driver => false,
            Self::Runtime | Self::Vm => true,
        }
    }

    const fn cargo_file(self) -> &'static str {
        match self {
            Self::Driver => "libklf_frontend_rt.a",
            Self::Runtime => "libklf_native_rt.a",
            Self::Vm => "libklf_vm_rt.a",
        }
    }
}

/// The bytecode runner `klf dist` built for `target`, `<driver>-run` for
/// every sysroot.
///
/// One runner serves every language: it reads the entry convention a language
/// declares from `KLF_VM_ENTRY_CONVENTION` when it starts, so no sysroot
/// compiles or links one.
///
/// # Errors
/// Returns a message when the distribution has no runner for `target`.
pub fn runner(target: &str) -> Result<PathBuf, String> {
    let Some(home) = archive_home() else {
        return Err(
            "no KLF distribution: the bytecode runner is not here, and building it needs \
             the workspace. Run `klf dist` once, or point KLF_HOME at a distribution."
                .to_owned(),
        );
    };
    let path = home.join("platforms").join(target).join(RUNNER);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "the KLF distribution at {} has no {} for `{target}`",
            home.display(),
            RUNNER
        ))
    }
}

/// The file name of the bytecode runner inside a platform.
pub const RUNNER: &str = "klfvm-run";

/// The distribution `klf` runs from: the one `KLF_HOME` names, or the one
/// this executable sits in. A `klf` built in the workspace sits in none, and
/// that is what lets `klf dist` build one.
#[must_use]
pub fn distribution_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("KLF_HOME") {
        return Some(PathBuf::from(home));
    }
    let executable = std::env::current_exe().ok()?;
    let home = executable.parent()?.parent()?.to_path_buf();
    holds_platform(&home).then_some(home)
}

/// The distribution whose archives to link: the one `klf` runs from, or the
/// newest `knvm` installed. A `klf` built in the workspace links the archives a
/// distributed one was built from rather than building its own, so that every
/// compile of a Kira program does not run Cargo and a user with neither `klf`
/// nor Rust on the path can use the toolchain at all.
#[must_use]
pub fn archive_home() -> Option<PathBuf> {
    distribution_home().or_else(installed_distribution)
}

/// The newest distribution `knvm` installed, newest by version rather than by
/// name, so `0.10.0` beats `0.9.0` however the two are spelled. An entry that
/// cannot be read is passed over rather than ending the search: the channel
/// directory holds a `current.toml` beside its versions, and one file that is
/// not a distribution is not a reason to find none.
fn installed_distribution() -> Option<PathBuf> {
    let root = klf_target::home_dir()?.join(".klf/toolchains/klf");
    let mut installed: Vec<(Vec<u64>, PathBuf)> = Vec::new();
    for channel in std::fs::read_dir(root).ok()?.flatten() {
        for version in std::fs::read_dir(channel.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = version.file_name().to_string_lossy().into_owned();
            // A dot names what an install is staging or has set aside.
            if !name.starts_with('.') {
                installed.push((version_key(&name), version.path()));
            }
        }
    }
    installed.sort_by(|left, right| left.0.cmp(&right.0));
    installed
        .into_iter()
        .rev()
        .map(|(_, path)| path)
        .find(|path| holds_platform(path))
}

/// Whether `home` is a distribution holding the host's platform.
fn holds_platform(home: &Path) -> bool {
    home.join("platforms")
        .join(host_triple())
        .join("services.manifest")
        .exists()
}

/// The path of `archive` for `target`.
///
/// # Errors
/// Returns a message when a distribution lacks the archive, or when building
/// it in the workspace fails.
pub fn kernel_archive(archive: KernelArchive, target: &str) -> Result<PathBuf, String> {
    if let Some(home) = archive_home() {
        let path = archive.distribution_path(&home, target);
        return if path.exists() {
            Ok(path)
        } else {
            Err(format!(
                "the KLF distribution at {} has no {} for `{target}`",
                home.display(),
                archive.file_name()
            ))
        };
    }
    // Nothing is built here. A `klf` without a distribution is a `klf` from
    // the workspace, and building its own runtime would make every compile of
    // a Kira program run Cargo — so a user with neither `klf` nor Rust on the
    // path could not use the toolchain at all. `klf dist` builds the archives
    // once; everything else reads them.
    Err(format!(
        "no KLF distribution: `{}` for `{target}` is not here, and building it \
         needs the workspace. Run `klf dist` once, or point KLF_HOME at a \
         distribution.",
        archive.file_name()
    ))
}

/// Builds `archive` for `target` with Cargo in the KLF workspace, optimized:
/// every generated compiler and program links it.
///
/// # Errors
/// Returns Cargo's output when the build fails, and a message when an archive
/// programs link cannot be stripped.
pub fn build_in_workspace(archive: KernelArchive, target: &str) -> Result<PathBuf, String> {
    let (mut command, directory) = klf_target::cargo_build(
        &workspace_root(),
        env!("CARGO").as_ref(),
        archive.package(),
        target,
    );
    configure_cross(&mut command, target)?;
    let output = command
        .current_dir(workspace_root())
        .output()
        .map_err(|error| format!("cargo build {}: {error}", archive.package()))?;
    if !output.status.success() {
        return Err(format!(
            "building {} for `{target}` failed\n{}",
            archive.package(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let built = directory.join(archive.cargo_file());
    if !archive.linked_by_programs() {
        return Ok(built);
    }
    let strip = find_llvm_tool("llvm-strip")
        .ok_or_else(|| "no bundled llvm-strip to prepare the runtime archives".to_owned())?;
    klf_target::without_debug_info(&strip, &built, &directory.join(archive.file_name()))
}

/// Points a Cargo build for a target this host is not at the bundled clang:
/// the C its dependencies compile (ring's, in the runtime) and the program it
/// links, the runner, take the target's C library from the sysroot
/// `KLF_SYSROOT_<TRIPLE>` or `KLF_SYSROOT` names. Left alone, cc-rs and the
/// linker run the host's `cc`, which on Darwin has no Linux headers and links
/// Mach-O, so a Tessera runtime never built and every compile for Tessera
/// fell back to building it in the workspace. An Apple target takes the SDK
/// its own toolchain finds.
///
/// # Errors
/// Returns a message when no bundled clang is found, or when a target that
/// needs a sysroot has none named.
pub fn configure_cross(command: &mut std::process::Command, target: &str) -> Result<(), String> {
    if target == host_triple() || target.contains("-apple-") {
        return Ok(());
    }
    let clang = find_clang().ok_or_else(|| format!("building for `{target}` needs the bundled clang"))?;
    let variable = format!("KLF_SYSROOT_{}", target.to_ascii_uppercase().replace('-', "_"));
    let sysroot = [variable.as_str(), "KLF_SYSROOT"]
        .iter()
        .find_map(|name| std::env::var_os(name).filter(|path| !path.is_empty()))
        .ok_or_else(|| {
            format!(
                "building for `{target}` needs a sysroot holding that target's C library; set {variable} or KLF_SYSROOT to it"
            )
        })?;
    let sysroot = PathBuf::from(sysroot);
    let machine = klf_target::machine_triple(target);
    let flags = format!("--target={machine} --sysroot={}", sysroot.display());
    command.env(format!("CC_{target}"), &clang);
    command.env(format!("CFLAGS_{target}"), &flags);
    if let Some(archiver) = find_llvm_tool("llvm-ar") {
        command.env(format!("AR_{target}"), archiver);
    }
    let key = target.to_ascii_uppercase().replace('-', "_");
    command.env(format!("CARGO_TARGET_{key}_LINKER"), &clang);
    command.env(
        format!("CARGO_TARGET_{key}_RUSTFLAGS"),
        format!(
            "-C link-arg=--target={machine} -C link-arg=--sysroot={} -C link-arg=-fuse-ld=lld",
            sysroot.display()
        ),
    );
    Ok(())
}

/// The triple `klf` itself runs on.
#[must_use]
pub fn host_triple() -> &'static str {
    klf_target::host_triple()
}

/// The bundled clang: `<home>/bin/clang` in a distribution, otherwise the
/// newest provisioned LLVM.
///
/// A distribution carries only the tools KLF runs — `clang`, `ld.lld` and
/// `llvm-strip` — beside its own binaries rather than a whole LLVM tree.
/// Nothing in KLF invokes `llc`, `opt`, or any other part of it.
#[must_use]
pub fn find_clang() -> Option<PathBuf> {
    find_llvm_tool("clang")
}

/// A tool KLF links with: `bin/<tool>` in a distribution, otherwise
/// `bin/<tool>` of the newest provisioned LLVM.
///
/// The tools sit in `bin` beside `klf`, so a distribution needs no LLVM tree
/// and nothing reaches outside its own directory to link a program.
#[must_use]
pub fn find_llvm_tool(tool: &str) -> Option<PathBuf> {
    let tool = klf_target::executable(tool);
    if let Some(home) = archive_home() {
        let path = home.join("bin").join(&tool);
        return path.exists().then_some(path);
    }
    provisioned_llvm()
        .map(|root| root.join("bin").join(&tool))
        .filter(|path| path.exists())
}

/// The newest RIDL LLVM: the directory holding `bin/clang` and the `lib`
/// directory its shared libraries are copied from.
#[must_use]
pub fn provisioned_llvm() -> Option<PathBuf> {
    klf_target::llvm_root().filter(|root| root.join("lib").is_dir())
}

/// Copies `libLLVM.so` beside the distribution's `bin`, which `ld.lld` loads
/// at startup. The unversioned name is copied as a real file rather than a
/// symlink so nothing in the distribution points outside itself.
fn copy_linker_library(out: &Path) -> Result<(), String> {
    let Some(root) = provisioned_llvm() else {
        return Err("no provisioned LLVM to take ld.lld's library from".to_owned());
    };
    let directory = out.join("lib");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let entries = std::fs::read_dir(root.join("lib"))
        .map_err(|error| format!("cannot read the provisioned LLVM: {error}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !is_shared_library(&name, "libLLVM") {
            continue;
        }
        let to = directory.join(name.as_ref());
        std::fs::copy(entry.path(), &to)
            .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
    }
    Ok(())
}

/// Whether `name` is a shared library of `stem` as the host spells one:
/// `libLLVM.so.23.1` on Linux, `libLLVM.23.1.dylib` or `libLLVM-23.dylib` on
/// macOS. A static `libLLVMCore.a` beside them is not one.
fn is_shared_library(name: &str, stem: &str) -> bool {
    let Some(rest) = name.strip_prefix(stem) else {
        return false;
    };
    (rest.starts_with('.') || rest.starts_with('-'))
        && (rest.contains(".so")
            || std::path::Path::new(rest)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dylib")))
}

/// Copies the shared libraries `bin/clang` loads beside it. It is a driver
/// over `libclang-cpp` and `libLLVM` rather than a static executable, so
/// without these it cannot start.
fn copy_toolchain_libraries(out: &Path) -> Result<(), String> {
    let Some(root) = provisioned_llvm() else {
        return Err("no provisioned LLVM to take clang's libraries from".to_owned());
    };
    let directory = out.join("lib");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    for stem in ["libclang-cpp", "libLLVM"] {
        let entries = std::fs::read_dir(root.join("lib"))
            .map_err(|error| format!("cannot read the provisioned LLVM: {error}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !is_shared_library(&name, stem) {
                continue;
            }
            let to = directory.join(name.as_ref());
            std::fs::copy(entry.path(), &to)
                .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
        }
    }
    // The headers clang carries rather than the system's — `stdarg.h`,
    // `stddef.h` and the rest. It looks for them beside itself, so a
    // distribution that ships `clang` without these cannot compile anything
    // that includes one.
    let resource = root.join("lib").join("clang");
    if resource.is_dir() {
        copy_tree(&resource, &directory.join("clang"))?;
    }
    Ok(())
}

/// Every file under `from`, keeping its layout under `to`.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to)
        .map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    let entries = std::fs::read_dir(from)
        .map_err(|error| format!("cannot read {}: {error}", from.display()))?;
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|error| format!("cannot copy {}: {error}", source.display()))?;
        }
    }
    Ok(())
}

/// Links `objects` and `archives` into `output` with the bundled `ld.lld`.
///
/// `clang` was a driver over `libclang-cpp` and `libLLVM`, which is why a
/// distribution that carried one had to carry 196M of libraries beside it. It
/// did nothing else here: KLF compiles no C, so the only work left was the
/// link, and a linker does that given the startup files and the C library
/// named outright.
///
/// # Errors
/// Returns the linker's output when the link fails, and a message when this
/// platform's startup files cannot be found.
pub fn link(
    objects: &[PathBuf],
    archives: &[PathBuf],
    output: &Path,
    _target: &str,
) -> Result<(), String> {
    // A compiler driver does more than hand the linker its arguments: it
    // knows the target's emulation, its C runtime and its unwind tables, and
    // picks startup files to match. Naming all of that here was meant to make
    // `ld.lld` sufficient on its own and did not — the binaries it produced
    // loaded and then faulted on their first indirect call. `clang` links
    // them correctly, so it stays the linker; `ld.lld` is what a
    // distribution carries for a caller that brings its own link line.
    let Some(clang) = find_clang() else {
        return Err("no clang under ~/.klf/llvm-ridl to link with".to_owned());
    };
    let mut command = Command::new(&clang);
    command.arg("-fuse-ld=lld");
    command.arg("-o").arg(output);
    if let Some(sysroot) = crate::native::macos_sysroot() {
        command.args(["-isysroot", &sysroot]);
    }
    for object in objects {
        command.arg(object);
    }
    for archive in archives {
        command.arg(archive);
    }
    // The shader validator and constant folder call libm.
    command.arg("-lm");
    let result = command
        .output()
        .map_err(|error| format!("ld.lld: {error}"))?;
    if result.status.success() {
        return Ok(());
    }
    Err(format!(
        "linking `{}` failed\n{}",
        output.display(),
        String::from_utf8_lossy(&result.stderr)
    ))
}

/// A version's components as numbers, so they order by magnitude. A component
/// that is not a number sorts after every one that is, which keeps an
/// unrecognised directory from reading as the newest.
fn version_key(name: &str) -> Vec<u64> {
    name.split(['.', '-'])
        .map(|component| component.parse().unwrap_or(u64::MAX))
        .collect()
}

/// Builds a KLF distribution at `out` (section 5.1): `bin/klf` beside the
/// tools it links with and the host's driver archive, and `platforms/<triple>/`
/// holding each target's runtime archives, service manifest and runner.
///
/// Nothing shader-related is bundled: every shader language is a language in
/// its own right, installed by `knvm` like `kira` and `ksl`.
///
/// # Errors
/// Returns a message when an archive does not build or a file cannot be
/// written.
pub fn build_distribution(out: &Path, targets: &[String]) -> Result<(), String> {
    if distribution_home().is_some() {
        return Err("`klf dist` runs from the KLF workspace, not from a distribution".into());
    }
    let create = |directory: &Path| {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))
    };
    let copy = |from: &Path, to: &Path| {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| format!("cannot copy {}: {error}", from.display()))
    };
    create(&out.join("bin"))?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    copy(&executable, &out.join("bin").join(klf_target::executable("klf")))?;

    // The linker and its two companions, which is all of the LLVM KLF runs. The
    // provisioned tree stays where it is; the distribution carries these files
    // rather than a link into a directory it does not own. No `clang`: KLF
    // compiles no C, and `ld.lld` is a single static binary that needs no
    // libraries beside it.
    // `ld64.lld` is the Mach-O linker clang runs for `-fuse-ld=lld` on macOS,
    // and `lld-link` the COFF one on Windows; lld reads which from its name.
    for tool in ["clang", "clang++", "ld.lld", "ld64.lld", "lld-link", "llvm-strip", "llvm-ar"] {
        if let Some(from) = find_llvm_tool(tool) {
            copy(&from, &out.join("bin").join(klf_target::executable(tool)))?;
        }
    }
    // `ld.lld` is a shared-library binary over `libLLVM`, so it needs this to
    // start. It is not optional the way `clang` was: there is no static linker
    // in the tree, and dropping it would mean dropping the managed linker for
    // whatever the machine happens to have.
    copy_linker_library(out)?;

    let host = host_triple();
    let driver_archive = KernelArchive::Driver;
    let driver = build_in_workspace(driver_archive, host)?;
    copy(&driver, &out.join("bin").join(driver_archive.file_name()))?;

    // `clang` is a driver over shared libraries beside it, not a static
    // executable: `libclang-cpp` is the compiler itself and `libLLVM` what it
    // links through. Without these it cannot start.
    copy_toolchain_libraries(out)?;

    for target in targets {
        let directory = out.join("platforms").join(target);
        create(&directory)?;
        copy(
            &workspace_root().join("kernel/services.manifest"),
            &directory.join("services.manifest"),
        )?;
        for archive in [KernelArchive::Runtime, KernelArchive::Vm] {
            let built = build_in_workspace(archive, target)?;
            copy(&built, &directory.join(archive.file_name()))?;
        }
        let runner = build_runner(target)?;
        copy(&runner, &directory.join(RUNNER))?;
    }
    // A shader language is a language like any other: it lives where its own
    // repository keeps it, and whoever builds a shader frontend names the
    // ones it needs. A distribution ships none of them, because a distribution
    // is `klf` and a user who never writes a shader should not carry a
    // megabyte of `.klfd` they will never read.
    Ok(())
}

/// Builds the bytecode runner for `target` with Cargo, once per triple.
///
/// The runner is an artifact rather than a recipe: a sysroot copies it, so
/// `klf build` never compiles or links one, and a user's machine needs no C
/// compiler and no Cargo to run bytecode.
fn build_runner(target: &str) -> Result<PathBuf, String> {
    let (mut command, directory) = klf_target::cargo_build(
        &workspace_root(),
        env!("CARGO").as_ref(),
        "klf-vm-rt",
        target,
    );
    let built = directory.join(RUNNER);
    if built.exists() {
        return Ok(built);
    }
    configure_cross(&mut command, target)?;
    command.args(["--bin", RUNNER]);
    let output = command
        .current_dir(workspace_root())
        .output()
        .map_err(|error| format!("cargo build {RUNNER}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "building the runner for `{target}` failed\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(built)
}
