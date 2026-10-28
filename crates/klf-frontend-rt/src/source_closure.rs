use klf_defsvc::FrontendMetadata;
use klf_services::{Session, effective_file_defaults};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub(crate) fn register(
    session: &mut Session,
    metadata: &FrontendMetadata,
    root: &Path,
    text: &str,
    known: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let std_include = crate::output::installed_sysroot()
        .map(|sysroot| crate::output::std_include(&sysroot, &metadata.name));
    let search = include_dirs(session);
    let mut pending = VecDeque::from([(root.to_owned(), text.to_owned())]);
    // `#define LIST "file.hh"`, for an `#include LIST` read after it.
    // Each value a name is given counts: a source may define the name, include
    // through it, and define it again for the next include.
    let mut named: std::collections::HashMap<String, Vec<(String, bool)>> =
        std::collections::HashMap::new();
    while let Some((path, source)) = pending.pop_front() {
        for (name, target) in include_names(&source) {
            named.entry(name).or_default().push(target);
        }
        let mut found = includes(&source);
        for name in computed_includes(&source) {
            if let Some(targets) = named.get(&name) {
                found.extend(targets.iter().cloned());
            }
        }
        for (requested, angled) in found {
            let Some(include) = resolve(&path, &requested, angled, &search, std_include.as_deref()) else {
                continue;
            };
            let include = normalized(&include);
            let identity = std::fs::canonicalize(&include).unwrap_or_else(|_| include.clone());
            if !known.insert(identity) {
                continue;
            }
            let include_text = session
                .read_text(&include)
                .map_err(|error| format!("cannot read {}: {error}", include.display()))?
                .to_string();
            let file = session.add_file(include.to_string_lossy().as_ref(), include_text.clone());
            let (memory, convention) =
                effective_file_defaults(metadata.memory, metadata.convention, None);
            session
                .set_file_defaults(file, memory, convention)
                .map_err(|error| format!("{}: {error}", include.display()))?;
            session
                .set_file_exclusivity(file, metadata.exclusivity)
                .map_err(|error| format!("{}: {error}", include.display()))?;
            pending.push_back((include, include_text));
        }
    }
    Ok(())
}

/// Each `#define NAME "file"` or `#define NAME <file>`: a name an
/// `#include NAME` may use.
fn include_names(source: &str) -> Vec<(String, (String, bool))> {
    let mut found = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("define") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            continue;
        }
        let (name, value) = rest.split_at(end);
        let value = value.trim();
        let (open, close) = match value.chars().next() {
            Some('"') => ('"', '"'),
            Some('<') => ('<', '>'),
            _ => continue,
        };
        let body = &value[open.len_utf8()..];
        if let Some(stop) = body.find(close)
            && stop > 0
            && body[stop + close.len_utf8()..].trim().is_empty()
        {
            found.push((name.to_owned(), (body[..stop].to_owned(), open == '<')));
        }
    }
    found
}

/// The macro each `#include NAME` names.
fn computed_includes(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("include") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim();
        if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            found.push(rest.to_owned());
        }
    }
    found
}

/// Each `#include` target, and whether it is written `<...>`.
fn includes(source: &str) -> Vec<(String, bool)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix("include") else {
            continue;
        };
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let Some(open) = rest.chars().next() else {
            continue;
        };
        let close = match open {
            '"' => '"',
            '<' => '>',
            _ => continue,
        };
        let body = &rest[open.len_utf8()..];
        let Some(end) = body.find(close) else {
            continue;
        };
        if end != 0 {
            found.push((body[..end].to_owned(), open == '<'));
        }
    }
    found
}

/// `a/b/../c` as `a/c`, without asking the file system: the name the
/// definition's preprocessor gives the same include.
fn normalized(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The directories a language's repeatable `include-dir` driver flag names,
/// one per line, as clang's `-I` takes them.
fn include_dirs(session: &Session) -> Vec<PathBuf> {
    match session.flag("include-dir") {
        Some(klf_services::DriverFlagValue::String(text)) => text
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// An include resolves next to the including file (only when written
/// `"..."`), then in each `include-dir` directory, then in the nearest
/// ancestor `include/` directory, then in the installed std's headers.
fn resolve(
    source: &Path,
    requested: &str,
    angled: bool,
    search: &[PathBuf],
    std_include: Option<&Path>,
) -> Option<PathBuf> {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let direct = parent.join(requested);
    if !angled && direct.is_file() {
        return Some(direct);
    }
    if let Some(found) = search
        .iter()
        .map(|directory| directory.join(requested))
        .find(|candidate| candidate.is_file())
    {
        return Some(found);
    }
    let mut ancestor = Some(parent);
    while let Some(directory) = ancestor {
        let candidate = directory.join("include").join(requested);
        if candidate.is_file() {
            return Some(candidate);
        }
        ancestor = directory.parent();
    }
    std_include
        .map(|directory| directory.join(requested))
        .filter(|candidate| candidate.is_file())
}
