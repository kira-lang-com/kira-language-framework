//! Reading corpus manifests: each `construct ... extends Corpus` and its
//! `CorpusCase` literals, from the syntax alone.

use klf_kira_syntax::ast::{CMember, DeclKind, Expr, ExprKind, FieldInit, MemberKind};
use klf_services::Session;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compared {
    Accept,
    Diagnostics,
    /// The full text a failed build printed, with the working directory's
    /// prefix removed from paths: the rendered diagnostics, headline through
    /// notes.
    Rendered,
    Stdout,
    Exit,
}

#[derive(Clone, Debug)]
pub struct CorpusCase {
    pub name: String,
    pub sources: Vec<String>,
    pub arguments: Vec<String>,
    pub accept: bool,
    pub diagnostics: Vec<String>,
    pub rendered: String,
    pub stdout: String,
    pub exit: i64,
    pub c_archives: Vec<String>,
    pub kira_archives: Vec<String>,
    pub kira_images: Vec<String>,
    pub engine: Engine,
    /// The operating systems the case runs on (`std::env::consts::OS`), or
    /// every one when empty.
    pub hosts: Vec<String>,
}

/// What runs a case's program: the target itself, or the language's runner
/// executing the image the driver emits for the VM.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Engine {
    #[default]
    Native,
    Vm,
}

#[derive(Clone, Debug)]
pub struct CorpusManifest {
    pub name: String,
    pub directory: PathBuf,
    pub oracle: Vec<String>,
    pub compare: Vec<Compared>,
    /// The other shader languages this corpus's cases ask targets of, as
    /// paths relative to the package. The driver these cases run against holds
    /// each of them, so a case naming `--target wgsl` is answered by an out
    /// rather than by a refusal that no out writes it.
    pub shader_languages: Vec<String>,
    pub cases: Vec<CorpusCase>,
}

/// The manifests under `<package>/corpus`, or only `<name>.klfd`.
///
/// # Errors
/// Returns a message when a manifest cannot be read or does not follow the
/// contract above, or when `only` names no manifest.
pub fn read_corpora(package: &Path, only: Option<&str>) -> Result<Vec<CorpusManifest>, String> {
    let directory = package.join("corpus");
    // A package may hold no corpus at all — an out with cases only for another
    // language's tests is one. Asked for a corpus by name, that is a name here
    // that does not exist.
    if !directory.is_dir() {
        return match only {
            Some(name) => Err(format!(
                "{}: no corpus manifest {name}.klfd",
                directory.display()
            )),
            None => Ok(Vec::new()),
        };
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
    if paths.is_empty() {
        return Err(match only {
            Some(name) => format!("no corpus manifest {}/{name}.klfd", directory.display()),
            None => format!("{} holds no corpus manifests", directory.display()),
        });
    }
    paths.iter().map(|path| read_manifest(path)).collect()
}

fn read_manifest(path: &Path) -> Result<CorpusManifest, String> {
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
            "needs exactly one `construct ... extends Corpus`".into(),
        ));
    };
    let mut oracle = Vec::new();
    let mut compare = vec![Compared::Accept, Compared::Stdout, Compared::Exit];
    // Diagnostics are compared by default: an expectation nothing checks is
    // no expectation. An oracle records no diagnostics, so it opts out.
    let mut compare_explicit = false;
    let mut shader_languages = Vec::new();
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
            "oracle" => oracle = strings(value).map_err(fail)?,
            "compare" => {
                compare_explicit = true;
                compare = strings(value)
                    .map_err(fail)?
                    .iter()
                    .map(|item| compared(item))
                    .collect::<Result<_, _>>()
                    .map_err(fail)?;
            }
            "shaderLanguages" => shader_languages = strings(value).map_err(fail)?,
            "cases" => cases = Some(corpus_cases(value).map_err(fail)?),
            other => return Err(fail(format!("unknown Corpus member `{other}`"))),
        }
    }
    let cases = cases.ok_or_else(|| fail("needs a `cases` member".into()))?;
    if oracle.is_empty() && !compare_explicit {
        compare.push(Compared::Diagnostics);
    }
    if !oracle.is_empty()
        && compare
            .iter()
            .any(|compared| matches!(compared, Compared::Diagnostics | Compared::Rendered))
    {
        return Err(fail(
            "`diagnostics` and `rendered` are compared against recorded expectations only".into(),
        ));
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let directory = std::path::absolute(parent)
        .map_err(|error| fail(format!("cannot resolve {}: {error}", parent.display())))?;
    Ok(CorpusManifest {
        name: construct.name.name.to_string(),
        directory,
        oracle,
        compare,
        shader_languages,
        cases,
    })
}

/// Whether `text` is `<code>:<line>`, both all digits: the form `diagnostic`
/// in `mod.rs` reports, so an expectation that could never match fails here.
fn is_expected_diagnostic(text: &str) -> bool {
    text.split_once(':').is_some_and(|(code, line)| {
        !code.is_empty()
            && !line.is_empty()
            && code.bytes().all(|byte| byte.is_ascii_digit())
            && line.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn compared(text: &str) -> Result<Compared, String> {
    Ok(match text {
        "accept" => Compared::Accept,
        "diagnostics" => Compared::Diagnostics,
        "rendered" => Compared::Rendered,
        "stdout" => Compared::Stdout,
        "exit" => Compared::Exit,
        other => return Err(format!("unknown compared item `{other}`")),
    })
}

fn corpus_cases(value: &Expr) -> Result<Vec<CorpusCase>, String> {
    let ExprKind::ArrayLit(items) = &value.kind else {
        return Err("`cases` must be an array of CorpusCase literals".into());
    };
    items.iter().map(corpus_case).collect()
}

fn corpus_case(value: &Expr) -> Result<CorpusCase, String> {
    let ExprKind::StructLit { fields, .. } = &value.kind else {
        return Err("`cases` entries must be CorpusCase literals".into());
    };
    let mut case = CorpusCase {
        name: String::new(),
        sources: Vec::new(),
        arguments: Vec::new(),
        accept: true,
        diagnostics: Vec::new(),
        rendered: String::new(),
        stdout: String::new(),
        exit: 0,
        c_archives: Vec::new(),
        kira_archives: Vec::new(),
        kira_images: Vec::new(),
        engine: Engine::Native,
        hosts: Vec::new(),
    };
    for (field, init) in fields {
        match field.name.as_str() {
            "name" => case.name = string(init)?,
            "sources" => case.sources = strings(init)?,
            "arguments" => case.arguments = strings(init)?,
            "accept" => {
                let ExprKind::Bool(accept) = init.kind else {
                    return Err("`accept` must be true or false".into());
                };
                case.accept = accept;
            }
            "diagnostics" => {
                let codes = strings(init)?;
                for code in &codes {
                    if !is_expected_diagnostic(code) {
                        return Err(format!(
                            "`diagnostics` entries are `<code>:<line>` with digits, got `{code}`"
                        ));
                    }
                }
                case.diagnostics = codes;
            }
            "rendered" => case.rendered = string(init)?,
            "stdout" => case.stdout = string(init)?,
            "exit" => case.exit = integer(init)?,
            "cArchives" => case.c_archives = strings(init)?,
            "kiraArchives" => case.kira_archives = strings(init)?,
            "kiraImages" => case.kira_images = strings(init)?,
            "hosts" => case.hosts = strings(init)?,
            "engine" => {
                case.engine = match string(init)?.as_str() {
                    "native" => Engine::Native,
                    "vm" => Engine::Vm,
                    other => return Err(format!("unknown engine `{other}`")),
                };
            }
            other => return Err(format!("unknown CorpusCase field `{other}`")),
        }
    }
    if case.name.is_empty() || case.sources.is_empty() {
        return Err("a CorpusCase needs a name and at least one source".into());
    }
    Ok(case)
}

fn string(value: &Expr) -> Result<String, String> {
    match &value.kind {
        ExprKind::Str(text) => Ok(text.clone()),
        _ => Err("expected a string literal".into()),
    }
}

fn strings(value: &Expr) -> Result<Vec<String>, String> {
    let ExprKind::ArrayLit(items) = &value.kind else {
        return Err("expected an array of string literals".into());
    };
    items.iter().map(string).collect()
}

fn integer(value: &Expr) -> Result<i64, String> {
    let (text, negative) = match &value.kind {
        ExprKind::Int(text) => (text.as_str(), false),
        ExprKind::Unary { operand, .. } => match &operand.kind {
            ExprKind::Int(text) => (text.as_str(), true),
            _ => return Err("expected an integer literal".into()),
        },
        _ => return Err("expected an integer literal".into()),
    };
    let value: i64 = text
        .replace('_', "")
        .parse()
        .map_err(|_| "integer literal out of range".to_owned())?;
    Ok(if negative { -value } else { value })
}
