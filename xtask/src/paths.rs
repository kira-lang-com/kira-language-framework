//! Where the workspace is.

use std::path::{Path, PathBuf};

/// The framework workspace: this crate's parent.
pub fn framework() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .to_path_buf()
}
