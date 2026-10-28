//! Reading editor corpus manifests (`KLF-EDITOR.md` Appendix A): each
//! `corpus/editor/<name>.klfd` holds one `construct ... extends
//! EditorCorpus` whose `cases` are `EditorCase` literals, read from the
//! syntax alone.
//!
//! The manifests are KLF Kira, so they follow grammar A.1: a struct literal
//! separates its fields with commas and a call labels an argument with
//! `label:`. A check's arguments may also be given in the order Appendix A
//! lists them.

use klf_kira_syntax::ast::{Arg, CMember, DeclKind, Expr, ExprKind, FieldInit, MemberKind};
use klf_services::Session;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorManifest {
    pub name: String,
    pub directory: PathBuf,
    pub cases: Vec<EditorCase>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorCase {
    pub name: String,
    pub files: Vec<EditorFile>,
    pub checks: Vec<EditorCheck>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorFile {
    pub path: String,
    pub text: String,
}

/// One check of Appendix A. `at` and the targets name a marker: `$0` the
/// cursor, any other name a `$<name>` position or a `$[name` range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorCheck {
    Diagnostics(Vec<String>),
    Definition {
        at: String,
        target: String,
    },
    References {
        at: String,
        targets: Vec<String>,
        include_declaration: bool,
    },
    Hover {
        at: String,
        contains: Vec<String>,
    },
    Completes {
        at: String,
        includes: Vec<String>,
        excludes: Vec<String>,
    },
    Signature {
        at: String,
        label: String,
        active_parameter: i64,
    },
    Rename {
        at: String,
        new_name: String,
        expect: Vec<EditorFile>,
    },
    Tokens(String),
    Symbols(String),
    Folds(Vec<String>),
    Hints(String),
    Action {
        at: String,
        title: String,
        expect: Vec<EditorFile>,
    },
    Format(Vec<EditorFile>),
    Edit {
        at: String,
        insert: String,
        then: Vec<EditorCheck>,
    },
}

/// The manifests under `<package>/corpus/editor`, or only `<name>.klfd`;
/// none when the package has no editor corpus.
///
/// # Errors
/// Returns a message when a manifest cannot be read or breaks the contract
/// above.
pub fn read_editor_corpora(
    package: &Path,
    only: Option<&str>,
) -> Result<Vec<EditorManifest>, String> {
    let directory = package.join("corpus").join("editor");
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "klfd")
        })
        .filter(|path| only.is_none_or(|name| path.file_stem().is_some_and(|stem| stem == name)))
        .collect();
    paths.sort();
    paths.iter().map(|path| read_manifest(path)).collect()
}

fn read_manifest(path: &Path) -> Result<EditorManifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut session = Session::new("aarch64-apple-darwin".parse().map_err(|_| "target")?);
    let file = session.add_file(path.to_string_lossy().as_ref(), text);
    let parsed = klf_kira_syntax::parse(session.file(file));
    let fail = |message: String| format!("{}: {message}", path.display());
    if let Some(diagnostic) = parsed.diagnostics.first() {
        return Err(fail(format!("does not parse: {}", diagnostic.message)));
    }
    let constructs: Vec<_> = parsed
        .file
        .decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Construct(construct) => Some(construct),
            _ => None,
        })
        .collect();
    let [construct] = constructs.as_slice() else {
        return Err(fail(
            "needs exactly one `construct ... extends EditorCorpus`".into(),
        ));
    };
    let mut cases = None;
    for member in &construct.members {
        let CMember::Member(member) = member else {
            continue;
        };
        let MemberKind::Field { name, init, .. } = &member.kind else {
            continue;
        };
        let Some(FieldInit::Expr(value)) = init else {
            continue;
        };
        match name.name.as_str() {
            "cases" => cases = Some(list(value, editor_case).map_err(fail)?),
            other => return Err(fail(format!("unknown EditorCorpus member `{other}`"))),
        }
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    Ok(EditorManifest {
        name: construct.name.name.to_string(),
        directory: std::path::absolute(parent)
            .map_err(|error| fail(format!("cannot resolve {}: {error}", parent.display())))?,
        cases: cases.ok_or_else(|| fail("needs a `cases` member".into()))?,
    })
}

fn list<T>(value: &Expr, item: fn(&Expr) -> Result<T, String>) -> Result<Vec<T>, String> {
    let ExprKind::ArrayLit(items) = &value.kind else {
        return Err("expected an array literal".into());
    };
    items.iter().map(item).collect()
}

fn fields<'a>(
    value: &'a Expr,
    what: &str,
) -> Result<&'a [(klf_kira_syntax::ast::Ident, Expr)], String> {
    match &value.kind {
        ExprKind::StructLit { fields, .. } => Ok(fields),
        _ => Err(format!("expected an {what} literal")),
    }
}

fn editor_case(value: &Expr) -> Result<EditorCase, String> {
    let mut case = EditorCase {
        name: String::new(),
        files: Vec::new(),
        checks: Vec::new(),
    };
    for (field, init) in fields(value, "EditorCase")? {
        match field.name.as_str() {
            "name" => case.name = string(init)?,
            "files" => case.files = list(init, editor_file)?,
            "checks" => case.checks = list(init, editor_check)?,
            other => return Err(format!("unknown EditorCase field `{other}`")),
        }
    }
    if case.name.is_empty() || case.files.is_empty() {
        return Err("an EditorCase needs a name and at least one file".into());
    }
    Ok(case)
}

fn editor_file(value: &Expr) -> Result<EditorFile, String> {
    let mut path = None;
    let mut text = None;
    for (field, init) in fields(value, "EditorFile")? {
        match field.name.as_str() {
            "path" => path = Some(string(init)?),
            "text" => text = Some(string(init)?),
            other => return Err(format!("unknown EditorFile field `{other}`")),
        }
    }
    Ok(EditorFile {
        path: path.ok_or("an EditorFile needs a `path`")?,
        text: text.ok_or("an EditorFile needs a `text`")?,
    })
}

/// The check `.Name(arguments)`.
fn editor_check(value: &Expr) -> Result<EditorCheck, String> {
    let ExprKind::EnumLit { name, args } = &value.kind else {
        return Err("a check is written `.Name(arguments)`".into());
    };
    let args = args.as_deref().unwrap_or_default();
    let check = name.name.as_str();
    let parameters: &[&str] = match check {
        "Diagnostics" => &["expected"],
        "Definition" => &["at", "target"],
        "References" => &["at", "targets", "includeDeclaration"],
        "Hover" => &["at", "contains"],
        "Completes" => &["at", "includes", "excludes"],
        "Signature" => &["at", "label", "activeParameter"],
        "Rename" => &["at", "newName", "expect"],
        "Tokens" | "Symbols" | "Hints" => &["golden"],
        "Folds" => &["ranges"],
        "Action" => &["at", "title", "expect"],
        "Format" => &["expect"],
        "Edit" => &["at", "insert", "then"],
        other => return Err(format!("unknown check `.{other}`")),
    };
    let a = arguments(check, args, parameters)?;
    Ok(match check {
        "Diagnostics" => EditorCheck::Diagnostics(strings(a[0])?),
        "Definition" => EditorCheck::Definition {
            at: string(a[0])?,
            target: string(a[1])?,
        },
        "References" => EditorCheck::References {
            at: string(a[0])?,
            targets: strings(a[1])?,
            include_declaration: boolean(a[2])?,
        },
        "Hover" => EditorCheck::Hover {
            at: string(a[0])?,
            contains: strings(a[1])?,
        },
        "Completes" => EditorCheck::Completes {
            at: string(a[0])?,
            includes: strings(a[1])?,
            excludes: strings(a[2])?,
        },
        "Signature" => EditorCheck::Signature {
            at: string(a[0])?,
            label: string(a[1])?,
            active_parameter: integer(a[2])?,
        },
        "Rename" => EditorCheck::Rename {
            at: string(a[0])?,
            new_name: string(a[1])?,
            expect: list(a[2], editor_file)?,
        },
        "Tokens" => EditorCheck::Tokens(string(a[0])?),
        "Symbols" => EditorCheck::Symbols(string(a[0])?),
        "Hints" => EditorCheck::Hints(string(a[0])?),
        "Folds" => EditorCheck::Folds(strings(a[0])?),
        "Action" => EditorCheck::Action {
            at: string(a[0])?,
            title: string(a[1])?,
            expect: list(a[2], editor_file)?,
        },
        "Format" => EditorCheck::Format(list(a[0], editor_file)?),
        _ => EditorCheck::Edit {
            at: string(a[0])?,
            insert: string(a[1])?,
            then: list(a[2], editor_check)?,
        },
    })
}

/// The argument for each of `parameters`, labeled or in order.
fn arguments<'a>(
    check: &str,
    args: &'a [Arg],
    parameters: &[&str],
) -> Result<Vec<&'a Expr>, String> {
    if args.len() != parameters.len() {
        return Err(format!(
            "`.{check}` takes {}: {}",
            parameters.len(),
            parameters.join(", ")
        ));
    }
    let mut found: Vec<Option<&Expr>> = vec![None; parameters.len()];
    for (index, arg) in args.iter().enumerate() {
        let slot = match &arg.label {
            Some(label) => parameters
                .iter()
                .position(|parameter| *parameter == label.name.as_str())
                .ok_or_else(|| format!("`.{check}` has no argument `{}`", label.name))?,
            None => index,
        };
        if found[slot].replace(&arg.value).is_some() {
            return Err(format!("`.{check}` is given `{}` twice", parameters[slot]));
        }
    }
    Ok(found.into_iter().flatten().collect())
}

fn string(value: &Expr) -> Result<String, String> {
    match &value.kind {
        ExprKind::Str(text) => Ok(text.clone()),
        _ => Err("expected a string literal".into()),
    }
}

fn strings(value: &Expr) -> Result<Vec<String>, String> {
    list(value, string)
}

fn boolean(value: &Expr) -> Result<bool, String> {
    match value.kind {
        ExprKind::Bool(value) => Ok(value),
        _ => Err("expected true or false".into()),
    }
}

fn integer(value: &Expr) -> Result<i64, String> {
    match &value.kind {
        ExprKind::Int(text) => text
            .as_str()
            .replace('_', "")
            .parse()
            .map_err(|_| "integer literal out of range".to_owned()),
        _ => Err("expected an integer literal".into()),
    }
}
