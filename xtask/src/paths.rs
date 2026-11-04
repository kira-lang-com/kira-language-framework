//! Where the workspace is.

use std::path::{Path, PathBuf};

/// The framework workspace: this crate's parent.
pub fn framework() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .to_path_buf()
}

/// Cargo's target directory for this workspace, honoring `CARGO_TARGET_DIR`.
pub fn target_directory(workspace: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || workspace.join("target"),
        |path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        },
    )
}
