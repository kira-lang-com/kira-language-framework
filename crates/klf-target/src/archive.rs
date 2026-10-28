//! Kernel archives as programs link them.
//!
//! Cargo's archive carries the debug information of every object in it, the
//! standard library's included. That is most of its size, and a linker reads
//! it and copies it into every program. The archive a program links goes
//! without it, so a link reads less and an executable stays small.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `built` without its debug information, at `linked`. `strip` (the bundled
/// `llvm-strip`) makes it again whenever `built` is newer.
///
/// # Errors
/// Returns a message when `strip` cannot run or fails, or the result cannot
/// be put in place.
pub fn without_debug_info(strip: &Path, built: &Path, linked: &Path) -> Result<PathBuf, String> {
    static STAGED: AtomicUsize = AtomicUsize::new(0);
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    };
    if let (Some(stripped), Some(source)) = (modified(linked), modified(built))
        && stripped >= source
    {
        return Ok(linked.to_path_buf());
    }
    // A concurrent link may read `linked` meanwhile, so it only ever changes
    // by a rename, and each call stages its own copy (`STAGED` numbers them):
    // links in other threads of this process may be making it too.
    let mut staged = linked.as_os_str().to_owned();
    staged.push(format!(
        ".{}.{}",
        std::process::id(),
        STAGED.fetch_add(1, Ordering::Relaxed)
    ));
    let staged = PathBuf::from(staged);
    // `llvm-strip` reads no MSVC archive. Its debug information is CodeView,
    // which `lld-link` leaves out of a program unless asked for a PDB, so the
    // archive is linked as Cargo built it.
    if built.extension().is_some_and(|extension| extension == "lib") {
        std::fs::copy(built, &staged).map_err(|error| format!("cannot copy {}: {error}", built.display()))?;
        std::fs::rename(&staged, linked)
            .map_err(|error| format!("cannot write {}: {error}", linked.display()))?;
        return Ok(linked.to_path_buf());
    }
    let output = Command::new(strip)
        .arg("--strip-debug")
        .arg("-o")
        .arg(&staged)
        .arg(built)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", strip.display()))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "stripping {} failed\n{}",
            built.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::rename(&staged, linked)
        .map_err(|error| format!("cannot write {}: {error}", linked.display()))?;
    Ok(linked.to_path_buf())
}
