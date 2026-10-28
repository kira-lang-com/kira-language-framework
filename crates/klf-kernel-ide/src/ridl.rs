//! `.ridl` and `.ridlsig` files: parse diagnostics, and `@Source` as a
//! document link and a definition target (`KLF-EDITOR.md` section 15.3).
//!
//! The place `@Source("<path>:<line>:<column>")` names is declared as an
//! entity in the file it points into, so the link and go to definition on
//! both the annotation and the declaration's name lead there.

use crate::Builder;
use crate::kir::entity;
use klf_facts::{LinkTarget, ReferenceRole, Symbol};
use klf_ridl::module::AnnValue;
use std::path::{Component, Path, PathBuf};

pub(crate) fn file(builder: &mut Builder<'_>, path: &Path) {
    let Some(text) = builder.text(path) else {
        return;
    };
    let file = builder.file(path);
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let (module, spans) = match klf_ridl::parse_spanned(file, name, &text) {
        Ok(parsed) => parsed,
        Err(diagnostic) => {
            builder.report(&diagnostic, &|_| Some(file));
            return;
        }
    };
    let directory = path.parent().unwrap_or(Path::new(""));
    for (item, spans) in module.items.iter().zip(&spans) {
        let sources = item
            .annotations
            .iter()
            .filter(|annotation| annotation.name == "Source")
            .filter_map(|annotation| match &annotation.args.first()?.value {
                AnnValue::String(source) => Some(source.as_str()),
                _ => None,
            });
        for (index, (source, span)) in sources.zip(&spans.sources).enumerate() {
            let Some((target, line, column)) = place(source) else {
                continue;
            };
            let target = normalized(&directory.join(target));
            let Some(entity) = declare(builder, &target, line, column) else {
                continue;
            };
            builder.facts.link(
                file,
                span.start + 1,
                span.end - 1,
                LinkTarget::Entity(entity),
            );
            builder.facts.reference(
                file,
                span.start + 1,
                span.end - 1,
                entity,
                ReferenceRole::Read,
            );
            if index == 0
                && let Some(name) = spans.name
            {
                builder
                    .facts
                    .reference(file, name.start, name.end, entity, ReferenceRole::Read);
            }
        }
    }
}

/// `path` with `.` and `..` folded away, so the file is named as the
/// editor names it.
fn normalized(path: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if folded.file_name().is_some() => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    folded
}

/// `<path>:<line>:<column>`, the line and column counted from 1.
fn place(source: &str) -> Option<(&str, u32, u32)> {
    let (rest, column) = source.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    Some((path, line.parse().ok()?, column.parse().ok()?))
}

/// The entity at `line` and `column` (a byte column, as diagnostics count)
/// of `path`, covering the name that starts there.
fn declare(
    builder: &mut Builder<'_>,
    path: &Path,
    line: u32,
    column: u32,
) -> Option<klf_facts::Entity> {
    let text = builder.text(path)?;
    let line_start = if line <= 1 {
        0
    } else {
        text.match_indices('\n')
            .nth(usize::try_from(line - 2).ok()?)
            .map(|(newline, _)| newline + 1)?
    };
    let start = line_start + usize::try_from(column.checked_sub(1)?).ok()?;
    let rest = text.get(start..)?;
    let length = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let file = builder.file(path);
    let start = u32::try_from(start).ok()?;
    let end = start + u32::try_from(length).ok()?;
    let entity = entity(file, start);
    builder.facts.declare_symbol(
        entity,
        Symbol {
            name: rest[..length].to_owned(),
            file: Some(file),
            start,
            end,
            selection_start: start,
            selection_end: end,
            ..Symbol::default()
        },
    );
    Some(entity)
}
