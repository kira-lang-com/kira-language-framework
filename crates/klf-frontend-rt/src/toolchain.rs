//! The C compiler and linker driver a target builds with: the bundled clang
//! for every target, with `--target` spelling the triple. The bundled clang
//! carries no C library of its own, so what differs between targets is where
//! that library comes from: an Apple target takes the SDK `xcrun` names, a
//! target this host is not takes the sysroot `KLF_SYSROOT` names, and the
//! host's own target takes neither, because the host's C library is the
//! target's.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The targets this driver emits code for.
pub(crate) fn check_target(target: &str) -> Result<(), String> {
    if klf_cranelift::SUPPORTED_TARGETS.contains(&klf_target::machine_triple(target).as_str()) {
        Ok(())
    } else {
        Err(format!(
            "unsupported target `{target}`; supported: {}",
            klf_cranelift::SUPPORTED_TARGETS.join(", ")
        ))
    }
}

/// Whether `target` is an Apple platform: Mach-O, `libSystem` and an SDK
/// from `xcrun`.
pub(crate) fn is_apple(target: &str) -> bool {
    target.contains("-apple-")
}

pub(crate) fn is_windows(target: &str) -> bool {
    target.contains("-windows-")
}

/// Where a tool of the bundled LLVM is: `$KLF_HOME/bin/<tool>` (or
/// `$KLF_HOME/llvm/bin`), else the `bin` of the newest KLF distribution `knvm`
/// installed, the same one the runtime archives come from, else the newest
/// provisioned LLVM. A driver on a machine that only ever ran `knvm install`
/// has the distribution and nothing else, and its clang must be the one its
/// runtime was linked for.
pub(crate) fn find_tool(tool: &str) -> Option<PathBuf> {
    let tool = &klf_target::executable(tool);
    if let Some(home) = std::env::var_os("KLF_HOME") {
        let home = PathBuf::from(home);
        return [
            home.join("bin").join(tool),
            home.join("llvm/bin").join(tool),
        ]
        .into_iter()
        .find(|path| path.exists());
    }
    if let Some(path) = crate::runtime::installed_distributions()
        .into_iter()
        .map(|home| home.join("bin").join(tool))
        .find(|path| path.exists())
    {
        return Some(path);
    }
    klf_target::llvm_root()
        .map(|root| root.join("bin").join(tool))
        .filter(|path| path.exists())
}

/// The SDK `xcrun` names for the Apple platform of `target`.
pub(crate) fn apple_sdk(target: &str) -> Option<String> {
    let sdk = if target.ends_with("-apple-ios") {
        "iphoneos"
    } else {
        "macosx"
    };
    let output = Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!path.is_empty()).then_some(path)
}

pub(crate) struct CToolchain {
    program: PathBuf,
    leading: Vec<OsString>,
}

impl CToolchain {
    pub(crate) fn for_target(target: &str) -> Result<Self, String> {
        let program = find_tool("clang").ok_or_else(|| "no bundled clang found".to_owned())?;
        // A Tessera target links as its Linux triple (KLF-RIDL 5.8).
        let machine = klf_target::machine_triple(target);
        let mut leading = vec![OsString::from(format!("--target={machine}"))];
        if is_apple(target) {
            if let Some(sdk) = apple_sdk(target) {
                leading.push("-isysroot".into());
                leading.push(sdk.into());
            }
        } else if target != klf_target::host_triple() {
            let sysroot = target_sysroot(target)?;
            // Checked here rather than left to the link: a sysroot with no
            // unwinder fails at the link as `unable to find library -lgcc_s`,
            // which names the linker rather than the sysroot short of one.
            if !has_unwinder(&sysroot) {
                return Err(format!(
                    "the sysroot at {} holds no `libgcc_s.so.1` or `libgcc_s.a`, which is the unwinder a `{target}` program links; install the target's GCC runtime into it",
                    sysroot.display()
                ));
            }
            leading.push("--sysroot".into());
            leading.push(sysroot.into());
        }
        Ok(Self { program, leading })
    }

    /// The driver with its target already selected.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.leading);
        command
    }

    pub(crate) fn run(&self, arguments: &[&OsStr]) -> Result<(), String> {
        let result = self
            .command()
            .args(arguments)
            .output()
            .map_err(|error| format!("{} failed: {error}", self.program.display()))?;
        if result.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{} failed\n{}",
                self.program.display(),
                String::from_utf8_lossy(&result.stderr)
            ))
        }
    }

    /// Hands this toolchain to cc-rs in a Cargo build for `target`, so the C a
    /// dependency compiles (ring's, in the runtime) finds the target's C
    /// library. Left alone, cc-rs runs the host's `cc` with no sysroot: on a
    /// Linux host a cross GCC carries its own, and on Darwin nothing does, so
    /// the build stops at `'assert.h' file not found`.
    pub(crate) fn configure_cc(&self, command: &mut Command, target: &str) {
        let flags: Vec<String> = self
            .leading
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        command.env(format!("CC_{target}"), &self.program);
        command.env(format!("CFLAGS_{target}"), flags.join(" "));
        if let Some(archiver) = find_tool("llvm-ar") {
            command.env(format!("AR_{target}"), archiver);
        }
    }

    /// What distinguishes this toolchain's output, for a cache key.
    pub(crate) fn identity(&self) -> Vec<u8> {
        let mut identity = self.program.as_os_str().as_encoded_bytes().to_vec();
        for argument in &self.leading {
            identity.push(0);
            identity.extend_from_slice(argument.as_encoded_bytes());
        }
        identity
    }
}

/// The library that unwinds a Rust panic through the runtime's frames. Darwin's
/// `libSystem` carries it, and so does MinGW's `libgcc_eh`, which rustc names
/// itself on Windows. On Linux it is the target's own GCC runtime, `libgcc_s`,
/// so the sysroot holding the target's C library has to hold that too.
pub(crate) fn unwinder(target: &str) -> &'static str {
    if is_apple(target) || is_windows(target) {
        ""
    } else {
        "-lgcc_s"
    }
}

/// The C library sysroot of a target this host is not.
///
/// This is the target's C library, not the sysroot an installed driver's
/// `--sysroot` flag names: that one is a toolchain layout (`bin/` and `lib/`)
/// the driver resolves from its own path, and it holds no C library.
fn target_sysroot(target: &str) -> Result<PathBuf, String> {
    let per_target = sysroot_variable(target);
    sysroot_from(target, |name| {
        std::env::var_os(name)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    })
    .ok_or_else(|| {
        format!(
            "building for `{target}` on `{}` needs a sysroot holding that target's C library; set {per_target} or KLF_SYSROOT to it",
            klf_target::host_triple()
        )
    })
}

/// `$KLF_SYSROOT_<TRIPLE>` for that target alone, else `$KLF_SYSROOT` for
/// whichever target is being built. Two names because a machine that builds for
/// two targets has two C libraries, and one of them is not the other's default.
fn sysroot_from(target: &str, mut lookup: impl FnMut(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    [sysroot_variable(target), "KLF_SYSROOT".to_owned()]
        .iter()
        .find_map(|name| lookup(name))
}

fn sysroot_variable(target: &str) -> String {
    format!(
        "KLF_SYSROOT_{}",
        target.to_ascii_uppercase().replace('-', "_")
    )
}

/// Whether a sysroot holds what `-lgcc_s` names: `libgcc_s.so.1` beside the
/// loader, or `libgcc_s.a` under `lib/gcc/<triple>/<version>`. Found by name
/// because a sysroot's layout is its distributor's. `libgcc_eh.a` does not
/// count: a program that links `-lgcc_s` does not link it, so a sysroot
/// holding only that one produces the linker error this check exists to name.
fn has_unwinder(sysroot: &Path) -> bool {
    fn walk(directory: &Path, depth: u32) -> bool {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                depth > 0 && walk(&path, depth - 1)
            } else {
                matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("libgcc_s.so.1" | "libgcc_s.so" | "libgcc_s.a")
                )
            }
        })
    }
    walk(sysroot, 5)
}

#[cfg(test)]
mod tests {
    use super::{has_unwinder, sysroot_from, sysroot_variable, unwinder};
    use std::path::{Path, PathBuf};

    #[test]
    fn a_linux_program_links_the_targets_own_unwinder() {
        assert_eq!(unwinder("aarch64-unknown-linux-gnu"), "-lgcc_s");
        assert_eq!(unwinder("x86_64-unknown-linux-gnu"), "-lgcc_s");
        assert_eq!(unwinder("aarch64-apple-darwin"), "");
        assert_eq!(unwinder("x86_64-pc-windows-gnu"), "");
    }

    #[test]
    fn a_cross_target_takes_its_sysroot_from_the_environment() {
        let mut asked = Vec::new();
        let per_target = sysroot_from("aarch64-unknown-linux-gnu", |name| {
            asked.push(name.to_owned());
            Some(PathBuf::from("/named"))
        })
        .unwrap();
        assert_eq!(per_target, Path::new("/named"));
        assert_eq!(
            asked,
            ["KLF_SYSROOT_AARCH64_UNKNOWN_LINUX_GNU".to_owned()],
            "the target's own variable answers, so the one for a single target is not asked"
        );

        asked.clear();
        let plain = sysroot_from("aarch64-unknown-linux-gnu", |name| {
            asked.push(name.to_owned());
            (name == "KLF_SYSROOT").then(|| PathBuf::from("/plain"))
        })
        .unwrap();
        assert_eq!(plain, Path::new("/plain"));
        assert_eq!(
            asked,
            [
                "KLF_SYSROOT_AARCH64_UNKNOWN_LINUX_GNU".to_owned(),
                "KLF_SYSROOT".to_owned()
            ]
        );

        assert!(
            sysroot_from("x86_64-pc-windows-gnu", |_| None).is_none(),
            "nothing answers for a target no variable names"
        );
    }

    #[test]
    fn a_sysroot_without_an_unwinder_is_told_so() {
        assert_eq!(
            sysroot_variable("aarch64-unknown-linux-gnu"),
            "KLF_SYSROOT_AARCH64_UNKNOWN_LINUX_GNU"
        );

        let root = std::env::temp_dir().join(format!("klf-sysroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("usr/lib/gcc/aarch64-linux-gnu/15"))
            .expect("staging directory");
        assert!(!has_unwinder(&root), "no unwinder in an empty sysroot");
        std::fs::write(
            root.join("usr/lib/gcc/aarch64-linux-gnu/15/libgcc_eh.a"),
            [],
        )
        .expect("libgcc_eh written");
        assert!(
            !has_unwinder(&root),
            "libgcc_eh.a is not what `-lgcc_s` names"
        );
        std::fs::write(root.join("usr/lib/libgcc_s.so.1"), []).expect("unwinder written");
        assert!(has_unwinder(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}
