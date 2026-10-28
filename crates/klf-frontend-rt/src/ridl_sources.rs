//! `--ridl-source` (`KLF-EDITOR.md` section 15.3): each declaration of an
//! emitted `.ridl` gets `@Source("<path>:<line>:<column>")`, the place in
//! the providing language the KIR function behind it was named, so an
//! editor can go from the `.ridl` to the source.

use crate::ridl_surface;
use klf_kir::Module;
use klf_services::Session;
use std::path::{Component, Path, PathBuf};

/// `text`, the `.ridl` written to `output`, with `@Source` before every
/// declaration whose function the session read. A declaration with no
/// function behind it (a type) and one already marked are left as they are.
pub(crate) fn mark(text: &str, module: &Module, session: &Session, output: &Path) -> String {
    let Ok((ridl, spans)) = klf_ridl::parse_spanned(klf_core::FileId::from_u32(0), "", text) else {
        return text.to_owned();
    };
    let entries = ridl_surface::entries(module).unwrap_or_default();
    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let directory = absolute(directory);
    let mut marks: Vec<(usize, String)> = Vec::new();
    for (item, span) in ridl.items.iter().zip(&spans) {
        if !span.sources.is_empty() {
            continue;
        }
        let path = klf_ridl::emit::item_path_of(item);
        let name = entries.get(&path).unwrap_or(&path);
        let Some(function) = module
            .functions
            .iter()
            .find(|function| function.name.as_str() == name.as_str() && !function.foreign)
        else {
            continue;
        };
        let Some(file) = session
            .files()
            .iter()
            .find(|file| file.id() == function.span.file)
        else {
            continue;
        };
        let at = file.line_column(function.span.start);
        let source = relative(&directory, &absolute(Path::new(file.name().as_str())));
        let start = span.start as usize;
        let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1);
        let indent = &text[line_start..start];
        let separator = if indent.trim().is_empty() {
            format!("\n{indent}")
        } else {
            " ".to_owned()
        };
        let source = source.replace('\\', "\\\\").replace('"', "\\\"");
        marks.push((
            start,
            format!("@Source(\"{source}:{}:{}\"){separator}", at.line, at.column),
        ));
    }
    let mut marked = text.to_owned();
    for (start, mark) in marks.into_iter().rev() {
        marked.insert_str(start, &mark);
    }
    marked
}

/// `path` with symbolic links resolved, so `/var` and `/private/var` on
/// macOS meet; as given, made absolute, when it does not exist.
fn absolute(path: &Path) -> PathBuf {
    path.canonicalize()
        .map(klf_target::plain_path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// `to` as a path from `from`, with `/` between its parts.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - shared];
    parts.extend(
        to[shared..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::relative;
    use std::path::Path;

    #[test]
    fn paths_are_relative_to_the_ridl_file() {
        assert_eq!(
            relative(Path::new("/w/out"), Path::new("/w/src/a.c")),
            "../src/a.c"
        );
        assert_eq!(relative(Path::new("/w"), Path::new("/w/a.c")), "a.c");
    }
}
