//! `definition.lock` (sections 5.4 and 5.6): what a sysroot was built from.

use crate::kernel::find_clang;
use klf_ridl::sha256::{hex, sha256};
use klf_services::DefinitionPackage;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The lock of a sysroot built from `package`. `files` are the std or
/// library files copied in, recorded relative to `root` under `kind`.
pub(crate) fn definition_lock<'a>(
    package: &DefinitionPackage,
    root: &Path,
    files: impl Iterator<Item = &'a PathBuf>,
    kind: &str,
    targets: &[&str],
) -> Result<String, String> {
    let mut lock = String::new();
    let _ = writeln!(lock, "klf {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(lock, "klfkira 1.0.0");
    let _ = writeln!(
        lock,
        "llvm {}",
        llvm_version().unwrap_or_else(|| "none".to_owned())
    );
    for target in targets {
        let _ = writeln!(lock, "target {target}");
    }
    for source in package.sources() {
        let _ = writeln!(
            lock,
            "definition {} {}",
            hex(&sha256(source.text.as_bytes())),
            source.name
        );
    }
    for path in files {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let name = path.strip_prefix(root).unwrap_or(path);
        let _ = writeln!(lock, "{kind} {} {}", hex(&sha256(&bytes)), name.display());
    }
    Ok(lock)
}

/// The bundled LLVM's version: the toolchain directory the clang lives in.
fn llvm_version() -> Option<String> {
    let clang = std::fs::canonicalize(find_clang()?).ok()?;
    let version = clang.parent()?.parent()?.parent()?.file_name()?;
    Some(version.to_string_lossy().into_owned())
}
