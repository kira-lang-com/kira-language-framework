use crate::FileMode;
use std::fmt;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileModeSelectionError {
    conflicting_modes: Vec<String>,
}

impl FileModeSelectionError {
    #[must_use]
    pub fn conflicting_modes(&self) -> &[String] {
        &self.conflicting_modes
    }
}

impl fmt::Display for FileModeSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "source matches multiple file modes: ")?;
        for (index, mode) in self.conflicting_modes.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "`{mode}`")?;
        }
        Ok(())
    }
}

impl std::error::Error for FileModeSelectionError {}

/// Selects the unique file mode matched by a path, pragma line, or flag glob.
/// Glob `*` matches any sequence of path characters and `?` matches one.
///
/// # Errors
/// Returns an error when more than one file mode matches.
pub fn select_file_mode<'a>(
    file_modes: &'a [FileMode],
    path: &Path,
    source: &str,
    flag_globs: &[(String, String)],
) -> Result<Option<&'a FileMode>, FileModeSelectionError> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    let path_text = path.to_str();
    let mut matching_modes = Vec::new();

    for (index, mode) in file_modes.iter().enumerate() {
        let extension_matches = extension.is_some_and(|extension| {
            mode.extensions
                .iter()
                .any(|candidate| candidate == extension)
        });
        let pragma = format!("#pragma {}", mode.pragma);
        let pragma_matches = source.lines().any(|line| line.trim() == pragma);
        let flag_matches = path_text.is_some_and(|path| {
            flag_globs
                .iter()
                .any(|(flag, glob)| flag == &mode.flag && glob_matches(glob, path))
        });

        if extension_matches || pragma_matches || flag_matches {
            matching_modes.push(index);
        }
    }

    match matching_modes.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(&file_modes[*index])),
        _ => {
            let mut conflicting_modes = matching_modes
                .into_iter()
                .map(|index| file_modes[index].name.clone())
                .collect::<Vec<_>>();
            conflicting_modes.sort();
            Err(FileModeSelectionError { conflicting_modes })
        }
    }
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let (mut pattern_index, mut value_index) = (0, 0);
    let mut star_index = None;
    let mut star_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(index) = star_index {
            star_value_index += 1;
            value_index = star_value_index;
            pattern_index = index + 1;
        } else {
            return false;
        }
    }

    pattern[pattern_index..]
        .iter()
        .all(|character| *character == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    enum Expected {
        Selected(&'static str),
        NoMatch,
        Conflict(&'static [&'static str]),
    }

    struct Case {
        name: &'static str,
        path: &'static str,
        source: &'static str,
        flag_globs: Vec<(String, String)>,
        expected: Expected,
    }

    #[test]
    fn selects_by_extension_pragma_and_flag_glob_or_reports_conflicts() {
        let file_modes = vec![
            FileMode {
                name: "raw".to_owned(),
                pragma: "karc raw".to_owned(),
                flag: "-fkarc-raw".to_owned(),
                extensions: vec!["raw".to_owned()],
                memory: None,
                convention: None,
            },
            FileMode {
                name: "arc".to_owned(),
                pragma: "karc arc".to_owned(),
                flag: "-fkarc-arc".to_owned(),
                extensions: vec!["arc".to_owned()],
                memory: None,
                convention: None,
            },
        ];
        let cases = [
            Case {
                name: "extension",
                path: "src/file.arc",
                source: "",
                flag_globs: vec![],
                expected: Expected::Selected("arc"),
            },
            Case {
                name: "trimmed pragma directive",
                path: "src/file.c",
                source: "  #pragma karc raw  \n",
                flag_globs: vec![],
                expected: Expected::Selected("raw"),
            },
            Case {
                name: "pragma with trailing tokens is not standalone",
                path: "src/file.c",
                source: "#pragma karc raw extra\n",
                flag_globs: vec![],
                expected: Expected::NoMatch,
            },
            Case {
                name: "star and question glob",
                path: "src/foo.c",
                source: "",
                flag_globs: vec![("-fkarc-arc".to_owned(), "src/f??.c".to_owned())],
                expected: Expected::Selected("arc"),
            },
            Case {
                name: "glob star spans path characters",
                path: "src/subdir/foo.c",
                source: "",
                flag_globs: vec![("-fkarc-arc".to_owned(), "src/*.c".to_owned())],
                expected: Expected::Selected("arc"),
            },
            Case {
                name: "flag name must match exactly",
                path: "src/foo.c",
                source: "",
                flag_globs: vec![("-fkarc-arc-extra".to_owned(), "src/*.c".to_owned())],
                expected: Expected::NoMatch,
            },
            Case {
                name: "same mode through multiple routes",
                path: "src/file.arc",
                source: "#pragma karc arc\n",
                flag_globs: vec![("-fkarc-arc".to_owned(), "src/*.arc".to_owned())],
                expected: Expected::Selected("arc"),
            },
            Case {
                name: "distinct matching modes conflict deterministically",
                path: "src/file.arc",
                source: "#pragma karc raw\n",
                flag_globs: vec![("-fkarc-raw".to_owned(), "src/*.arc".to_owned())],
                expected: Expected::Conflict(&["arc", "raw"]),
            },
        ];

        for case in cases {
            let result = select_file_mode(
                &file_modes,
                Path::new(case.path),
                case.source,
                &case.flag_globs,
            );
            match (result, case.expected) {
                (Ok(Some(mode)), Expected::Selected(name)) => {
                    assert_eq!(mode.name, name, "{}", case.name);
                }
                (Ok(None), Expected::NoMatch) => {}
                (Err(error), Expected::Conflict(names)) => {
                    assert_eq!(error.conflicting_modes(), names, "{}", case.name);
                    assert_eq!(
                        error.to_string(),
                        "source matches multiple file modes: `arc`, `raw`",
                        "{}",
                        case.name
                    );
                }
                (actual, _) => panic!("{}: unexpected result {actual:?}", case.name),
            }
        }
    }
}
