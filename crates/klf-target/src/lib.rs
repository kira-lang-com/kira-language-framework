//! Target triples and layout properties used by KIR, and the kernel
//! archives programs for a target link.

mod archive;
mod c_layout;

use std::fmt;
use std::str::FromStr;

pub use archive::without_debug_info;
pub use c_layout::{BitFieldRule, CLayout, CScalar, LongDouble};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Endianness {
    Little,
    Big,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Target {
    triple: String,
    pointer_width: u8,
    endianness: Endianness,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidTarget(pub String);

impl Target {
    #[must_use]
    pub fn triple(&self) -> &str {
        &self.triple
    }

    #[must_use]
    pub const fn pointer_width(&self) -> u8 {
        self.pointer_width
    }

    #[must_use]
    pub const fn endianness(&self) -> Endianness {
        self.endianness
    }
}

impl FromStr for Target {
    type Err = InvalidTarget;

    fn from_str(triple: &str) -> Result<Self, Self::Err> {
        let architecture = triple.split('-').next().unwrap_or_default();
        let (pointer_width, endianness) = match architecture {
            "aarch64" | "x86_64" | "riscv64" | "wasm64" => (64, Endianness::Little),
            "i386" | "i486" | "i586" | "i686" | "arm" | "riscv32" | "wasm32" => {
                (32, Endianness::Little)
            }
            "aarch64_be" | "powerpc64" | "s390x" => (64, Endianness::Big),
            "powerpc" => (32, Endianness::Big),
            _ => return Err(InvalidTarget(triple.to_owned())),
        };
        Ok(Self {
            triple: triple.to_owned(),
            pointer_width,
            endianness,
        })
    }
}

/// Whether `triple` names Tessera, `<arch>-unknown-tessera` (KLF-RIDL 5.8).
#[must_use]
pub fn is_tessera(triple: &str) -> bool {
    triple.ends_with("-unknown-tessera")
}

/// The triple machine code, the C toolchain and the linker work in for
/// `triple`: a Tessera triple's is the Linux triple of its architecture,
/// which is the LLVM target its custom Rust target names, and every other
/// triple's is itself (KLF-RIDL 5.8).
#[must_use]
pub fn machine_triple(triple: &str) -> String {
    match triple.strip_suffix("-unknown-tessera") {
        Some(architecture) => format!("{architecture}-unknown-linux-gnu"),
        None => triple.to_owned(),
    }
}

/// A Cargo build of `package` from the workspace at `workspace` for
/// `target`, and the directory it writes its release artifacts to.
///
/// The host's target builds with `cargo` alone and a target Rust ships builds
/// with `--target`. A Tessera target is a custom target,
/// `kernel/targets/<triple>.json`, whose standard library Rust does not ship,
/// so it builds with the nightly toolchain, which builds `std` for it from
/// source.
#[must_use]
pub fn cargo_build(
    workspace: &std::path::Path,
    cargo: &std::ffi::OsStr,
    package: &str,
    target: &str,
) -> (std::process::Command, std::path::PathBuf) {
    let mut command;
    if is_tessera(target) {
        command = std::process::Command::new("rustup");
        command.args(["run", "nightly", "cargo"]);
    } else {
        command = std::process::Command::new(cargo);
    }
    command.args(["build", "--quiet", "--release", "-p", package]);
    if target == host_triple() {
        return (command, workspace.join("target/release"));
    }
    if is_tessera(target) {
        command
            .args(["-Z", "build-std=std", "-Z", "json-target-spec", "--target"])
            .arg(workspace.join("kernel/targets").join(format!("{target}.json")));
    } else {
        command.args(["--target", target]);
    }
    (command, workspace.join("target").join(target).join("release"))
}

/// Where KLF keeps what it caches between runs on this machine: `KLF_CACHE`
/// when set, `%LOCALAPPDATA%\klf\cache` on Windows, `~/.cache/klf`
/// otherwise, and nowhere when `KLF_CACHE` is empty.
#[must_use]
pub fn cache_root() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("KLF_CACHE") {
        return (!path.is_empty()).then(|| std::path::PathBuf::from(path));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return Some(std::path::PathBuf::from(local).join("klf").join("cache"));
    }
    home_dir().map(|home| home.join(".cache/klf"))
}

/// The user's home directory: `HOME`, or `USERPROFILE` on Windows, where a
/// shell outside MSYS sets no `HOME`.
#[must_use]
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from)
}

/// `path` without the `\\?\` prefix Windows gives a canonical path, when a
/// plain drive path is behind it. A verbatim path takes no `/`, and a path
/// handed to a definition has its parts joined with one.
#[must_use]
pub fn plain_path(path: std::path::PathBuf) -> std::path::PathBuf {
    let plain = path
        .to_str()
        .and_then(|text| text.strip_prefix(r"\\?\"))
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .map(std::path::PathBuf::from);
    plain.unwrap_or(path)
}

/// `name` as an executable is spelled on this host: `clang.exe` on Windows.
#[must_use]
pub fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// The triple of the machine this code runs on.
#[must_use]
pub fn host_triple() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("aarch64", "windows") => "aarch64-pc-windows-msvc",
        _ => "unknown",
    }
}

/// Whether `target` is Windows with the MSVC toolchain, whose linker is
/// `lld-link` and whose static libraries are `name.lib`.
#[must_use]
pub fn is_msvc(target: &str) -> bool {
    target.ends_with("-windows-msvc")
}

/// The flag that makes code position-independent for `target`: `-fPIC`, and
/// none on MSVC, where every object is relocatable and clang refuses it.
#[must_use]
pub fn pic_flags(target: &str) -> &'static [&'static str] {
    if is_msvc(target) { &[] } else { &["-fPIC"] }
}

/// `name`'s static library as Cargo builds it for `target`:
/// `libname.a`, or `name.lib` on MSVC.
#[must_use]
pub fn static_library(name: &str, target: &str) -> String {
    if is_msvc(target) { format!("{name}.lib") } else { format!("lib{name}.a") }
}

/// What a Rust static library needs from Windows when linked with MSVC, as
/// `rustc --print native-static-libs` lists it, and the DLL C runtime it was
/// compiled for in place of the static one clang would pick.
pub const MSVC_SYSTEM_LIBRARIES: [&str; 12] = [
    "-lpsapi", "-lshell32", "-luser32", "-ladvapi32", "-lbcrypt", "-lkernel32",
    "-lntdll", "-luserenv", "-lws2_32", "-ldbghelp", "-lmsvcrt", "-Wl,/NODEFAULTLIB:libcmt",
];

/// The RIDL LLVM release this KLF links with, published by
/// `kira-lang-com/llvm-ridl` as `v<version>`.
pub const LLVM_RIDL_VERSION: &str = "23.1.0-ridl.1";

/// This machine's name in release asset names: `llvm-ridl-<version>-<host>`,
/// `klf-<version>-<host>`.
#[must_use]
pub fn llvm_host_key() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-macos",
        ("x86_64", "macos") => "x86_64-macos",
        ("aarch64", "linux") => "aarch64-linux-gnu",
        ("x86_64", "linux") => "x86_64-linux-gnu",
        ("x86_64", "windows") => "x86_64-windows-msvc",
        ("aarch64", "windows") => "aarch64-windows-msvc",
        _ => "unknown",
    }
}

/// The newest RIDL LLVM installed, `~/.klf/llvm-ridl/<version>`: the tree
/// holding `bin/clang` and the `lib` beside it. Versions order by their
/// numbers, so `23.1.0` is newer than `9.0.0`.
#[must_use]
pub fn llvm_root() -> Option<std::path::PathBuf> {
    let clang = executable("clang");
    let mut installed: Vec<(Vec<u64>, std::path::PathBuf)> =
        std::fs::read_dir(home_dir()?.join(".klf/llvm-ridl"))
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|root| root.join("bin").join(&clang).exists())
            .map(|root| {
                let name = root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let key = name.split(['.', '-']).map(|part| part.parse().unwrap_or(0)).collect();
                (key, root)
            })
            .collect();
    installed.sort();
    installed.pop().map(|(_, root)| root)
}

/// `bin/<tool>` of the newest RIDL LLVM, when it carries that tool.
#[must_use]
pub fn llvm_tool(tool: &str) -> Option<std::path::PathBuf> {
    llvm_root()
        .map(|root| root.join("bin").join(executable(tool)))
        .filter(|path| path.is_file())
}

impl fmt::Display for InvalidTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported target triple `{}`", self.0)
    }
}

impl std::error::Error for InvalidTarget {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tessera_triple_works_in_its_linux_triple() {
        assert!(is_tessera("aarch64-unknown-tessera"));
        assert_eq!(machine_triple("aarch64-unknown-tessera"), "aarch64-unknown-linux-gnu");
        assert_eq!(machine_triple("x86_64-unknown-linux-gnu"), "x86_64-unknown-linux-gnu");
        let target: Target = "aarch64-unknown-tessera".parse().unwrap();
        assert_eq!(target.pointer_width(), 64);
    }

    #[test]
    fn derives_pointer_width_from_architecture() {
        let target: Target = "aarch64-apple-darwin".parse().unwrap();
        assert_eq!(target.pointer_width(), 64);
        assert_eq!(target.endianness(), Endianness::Little);
    }
}
