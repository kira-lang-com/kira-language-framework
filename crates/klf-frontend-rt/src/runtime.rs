//! The kernel runtime a native program links: `libklfrt.a`, or, for a
//! program that links a RIDL module built for the VM, `libklfvm.a`, which
//! holds the VM beside the same runtime. The module's native library runs
//! its image when it loads, through `klfvm_attach`, and its entries call
//! into the image through `klfvm_invoke` (section 17.3), both of which the
//! program exports. A program that calls one of the VM's own C entry points,
//! a `klfvm_` symbol such as the live runtime device's, links `libklfvm.a`
//! too, since that archive is where they are defined.
//!
//! In a sysroot the archive is `<out>/lib/klf/<target>/<archive>`. Outside one
//! it is the KLF distribution's, `<home>/platforms/<target>/<archive>`, with
//! the home `$KLF_HOME` or the newest `knvm` installed. Only a compiler with
//! neither builds it in the KLF workspace, and it then keeps what it built in
//! the newest installed distribution, so Cargo runs once per target rather
//! than on every build.

use crate::native_link::LIBRARY_ATTRIBUTE;
use crate::output::installed_sysroot;
use crate::toolchain::{CToolchain, find_tool, is_apple, is_windows};
use klf_kir::Module;
use std::path::{Path, PathBuf};

/// The prefix of every C entry point `libklfvm.a` defines.
const VM_ENTRY_PREFIX: &str = "klfvm_";

#[derive(Clone, Copy)]
pub(crate) enum Runtime {
    Native,
    Vm,
}

impl Runtime {
    /// The runtime `module` links: the VM's when it links a `.klbc` or calls
    /// one of the VM's C entry points.
    pub(crate) fn of(module: &Module) -> Self {
        let calls_the_vm = module.functions.iter().any(|function| {
            function.foreign && function.name.as_str().starts_with(VM_ENTRY_PREFIX)
        });
        let links_image = module.attributes.iter().any(|attribute| {
            attribute.name == LIBRARY_ATTRIBUTE
                && attribute
                    .value
                    .as_deref()
                    .is_some_and(|value| value.ends_with(".klbc"))
        });
        if links_image || calls_the_vm {
            Self::Vm
        } else {
            Self::Native
        }
    }

    const fn file_name(self) -> &'static str {
        match self {
            Self::Native => "libklfrt.a",
            Self::Vm => "libklfvm.a",
        }
    }

    const fn package(self) -> &'static str {
        match self {
            Self::Native => "klf-native-rt",
            Self::Vm => "klf-vm-rt",
        }
    }

    fn cargo_file(self, target: &str) -> String {
        let name = match self {
            Self::Native => "klf_native_rt",
            Self::Vm => "klf_vm_rt",
        };
        klf_target::static_library(name, target)
    }

    /// What the program's link adds for the runtime besides its archive: the
    /// VM's entry points, kept and exported for the libraries that call them.
    pub(crate) fn link_arguments(self, target: &str) -> Vec<&'static str> {
        match self {
            Self::Native => Vec::new(),
            Self::Vm if is_apple(target) => vec![
                "-Wl,-u,_klfvm_invoke",
                "-Wl,-u,_klfvm_attach",
                "-Wl,-export_dynamic",
            ],
            Self::Vm if klf_target::is_msvc(target) => vec![
                "-Wl,/INCLUDE:klfvm_invoke",
                "-Wl,/INCLUDE:klfvm_attach",
                "-Wl,/EXPORT:klfvm_invoke",
                "-Wl,/EXPORT:klfvm_attach",
            ],
            Self::Vm if is_windows(target) => vec![
                "-Wl,-u,klfvm_invoke",
                "-Wl,-u,klfvm_attach",
                "-Wl,--export-all-symbols",
            ],
            Self::Vm => vec!["-Wl,-u,klfvm_invoke", "-Wl,-u,klfvm_attach", "-rdynamic"],
        }
    }

    /// The archive for `target`.
    pub(crate) fn archive(self, target: &str) -> Result<PathBuf, String> {
        if let Some(archive) = installed_sysroot()
            .map(|root| root.join("lib/klf").join(target).join(self.file_name()))
            .filter(|archive| archive.exists())
        {
            return Ok(archive);
        }
        if let Some(home) = std::env::var_os("KLF_HOME") {
            let path = self.in_distribution(&PathBuf::from(home), target);
            return if path.exists() {
                Ok(path)
            } else {
                Err(format!("{} does not exist", path.display()))
            };
        }
        let installed = installed_distributions();
        if let Some(archive) = installed
            .iter()
            .map(|home| self.in_distribution(home, target))
            .find(|archive| archive.exists())
        {
            return Ok(archive);
        }
        let built = self.built_in_workspace(target)?;
        // Kept in the newest distribution, so the next compile for this
        // target links it from there instead of running Cargo again.
        if let Some(home) = installed.first() {
            // Copied beside it and renamed into place, so a compile running
            // at the same time never links half an archive.
            let kept = self.in_distribution(home, target);
            let partial = kept.with_extension(format!("a.{}", std::process::id()));
            if kept
                .parent()
                .is_some_and(|directory| std::fs::create_dir_all(directory).is_ok())
                && std::fs::copy(&built, &partial).is_ok()
                && std::fs::rename(&partial, &kept).is_ok()
            {
                return Ok(kept);
            }
            let _ = std::fs::remove_file(&partial);
        }
        Ok(built)
    }

    /// Where the KLF distribution at `home` keeps the archive for `target`.
    fn in_distribution(self, home: &Path, target: &str) -> PathBuf {
        home.join("platforms").join(target).join(self.file_name())
    }

    fn built_in_workspace(self, target: &str) -> Result<PathBuf, String> {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let (mut command, directory) =
            klf_target::cargo_build(&workspace, env!("CARGO").as_ref(), self.package(), target);
        if target != klf_target::host_triple() {
            CToolchain::for_target(target)?.configure_cc(&mut command, target);
        }
        let result = command
            .current_dir(&workspace)
            .output()
            .map_err(|error| format!("cargo build {}: {error}", self.package()))?;
        if !result.status.success() {
            return Err(format!(
                "building {} for `{target}` failed\n{}",
                self.package(),
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        let strip = find_tool("llvm-strip")
            .ok_or_else(|| "no bundled llvm-strip to prepare the runtime archive".to_owned())?;
        klf_target::without_debug_info(
            &strip,
            &directory.join(self.cargo_file(target)),
            &directory.join(self.file_name()),
        )
    }
}

/// The KLF distributions `knvm` installed, newest version first, whatever
/// their channel: `~/.klf/toolchains/klf/<channel>/<version>`.
pub(crate) fn installed_distributions() -> Vec<PathBuf> {
    let Some(home) = klf_target::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".klf/toolchains/klf");
    let mut installed: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|channel| {
            std::fs::read_dir(channel.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        // A dot names what an install is staging or has set aside.
        .filter(|version| !version.file_name().to_string_lossy().starts_with('.'))
        .map(|version| {
            let name = version.file_name().to_string_lossy().into_owned();
            let key = name
                .split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect();
            (key, version.path())
        })
        .collect();
    installed.sort_by(|left, right| right.0.cmp(&left.0));
    installed.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::Runtime;

    fn runtime_of(declaration: &str) -> &'static str {
        let module = klf_kir::parse_module(&format!(
            "module @Shell lang \"test\" target \"x86_64-unknown-linux-gnu\" stage canonical\n\n{declaration}\n"
        ))
        .expect("module parses");
        Runtime::of(&module).file_name()
    }

    #[test]
    fn a_program_calling_the_vm_links_the_vm_runtime() {
        assert_eq!(
            runtime_of(
                "func private foreign @klfvm_live_start(%a: ptr, %b: ptr, %c: ptr) -> i64 engine(native)"
            ),
            "libklfvm.a"
        );
        assert_eq!(
            runtime_of("func private foreign @puts(%a: ptr) -> i32 engine(native)"),
            "libklfrt.a"
        );
    }
}
