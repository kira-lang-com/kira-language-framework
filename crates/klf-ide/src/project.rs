//! Which project a file belongs to (`KLF-EDITOR.md` section 9.2, the default
//! rule).

use std::path::{Path, PathBuf};

/// A set of inputs one `check` analyzes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    /// The uniform driver arguments of the `check`: the root directory, or the
    /// file itself for a loose file.
    pub arguments: Vec<String>,
    pub loose: bool,
}

/// The project of `file`: the nearest directory above it holding one of
/// `markers`, else the innermost workspace folder containing it, else the
/// file alone.
#[must_use]
pub fn resolve(file: &Path, folders: &[PathBuf], markers: &[String]) -> Project {
    let mut directory = file.parent();
    while let Some(candidate) = directory {
        if markers.iter().any(|marker| candidate.join(marker).exists()) {
            return directory_project(candidate);
        }
        directory = candidate.parent();
    }
    folders
        .iter()
        .filter(|folder| file.starts_with(folder))
        .max_by_key(|folder| folder.components().count())
        .map_or_else(
            || Project {
                root: file.parent().map_or_else(PathBuf::new, Path::to_path_buf),
                arguments: vec![file.to_string_lossy().into_owned()],
                loose: true,
            },
            |folder| directory_project(folder),
        )
}

fn directory_project(root: &Path) -> Project {
    Project {
        root: root.to_path_buf(),
        arguments: vec![root.to_string_lossy().into_owned()],
        loose: false,
    }
}
