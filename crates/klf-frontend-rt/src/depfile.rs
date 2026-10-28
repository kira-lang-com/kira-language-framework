//! `build --depfile <path>` (KLF-RIDL section 5.5): every path the compile
//! read, asked about or listed, as a make-style depfile a build system keys
//! the compile's discovered inputs on.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

/// Writes the paths noted since the build began as the prerequisites of
/// `target`, one to a line.
pub(crate) fn write(path: &Path, target: &Path) -> Result<(), String> {
    let mut text = escaped(target);
    text.push(':');
    let mut paths: Vec<PathBuf> = klf_services::accessed()
        .iter()
        .map(|path| plain(path))
        .collect();
    paths.sort();
    paths.dedup();
    for accessed in paths {
        let _ = write!(text, " \\\n  {}", escaped(&accessed));
    }
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// `path` without `.` or `..`: canonical where it exists, so a path read
/// through a link or from `../` names the file itself, else with each `..`
/// taking the name before it away.
fn plain(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let mut plain = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                plain.pop();
            }
            other => plain.push(other),
        }
    }
    plain
}

/// `path` as make reads it: a space, `#` and `\` escaped with `\`, and `$`
/// doubled.
fn escaped(path: &Path) -> String {
    let mut text = String::new();
    for character in path.to_string_lossy().chars() {
        match character {
            ' ' | '#' | '\\' => {
                text.push('\\');
                text.push(character);
            }
            '$' => text.push_str("$$"),
            _ => text.push(character),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{escaped, plain};
    use std::path::Path;

    #[test]
    fn paths_are_escaped_as_make_reads_them() {
        assert_eq!(escaped(Path::new("/a b/c#d/$e")), "/a\\ b/c\\#d/$$e");
    }

    #[test]
    fn missing_paths_lose_their_dots() {
        assert_eq!(
            plain(Path::new("/missing-klf/a/./b/../c")),
            Path::new("/missing-klf/a/c")
        );
    }
}
