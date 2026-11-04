//! Which project a file belongs to (`KLF-EDITOR.md` section 9.2, the default
//! rule).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDiagnostic {
    pub message: String,
    pub code: Option<i64>,
    pub span: Option<(u32, u32)>,
    pub path: Option<PathBuf>,
    pub severity: klf_diag::Severity,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectResolution {
    pub project: Option<Project>,
    pub diagnostics: Vec<ProjectDiagnostic>,
    pub failed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectError {
    pub message: String,
    pub code: Option<i64>,
    pub span: Option<(u32, u32)>,
    /// The source that caused the error, or the file being resolved.
    pub path: Option<PathBuf>,
}

impl From<String> for ProjectError {
    fn from(message: String) -> Self {
        Self {
            message,
            code: None,
            span: None,
            path: None,
        }
    }
}

impl From<&str> for ProjectError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// A set of inputs one `check` analyzes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
    /// The uniform driver arguments of the `check`: the root directory, or the
    /// file itself for a loose file.
    pub arguments: Vec<String>,
    pub loose: bool,
    pub watch: Vec<String>,
    pub output: Option<PathBuf>,
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
                name: String::new(),
                root: file.parent().map_or_else(PathBuf::new, Path::to_path_buf),
                arguments: vec![file.to_string_lossy().into_owned()],
                loose: true,
                watch: Vec::new(),
                output: None,
            },
            |folder| directory_project(folder),
        )
}

fn directory_project(root: &Path) -> Project {
    Project {
        name: root
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        root: root.to_path_buf(),
        arguments: vec![root.to_string_lossy().into_owned()],
        loose: false,
        watch: Vec::new(),
        output: None,
    }
}
