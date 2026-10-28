//! `klf editor gen`: the editor packages of a definition, or of the kernel's
//! own file types (`KLF-EDITOR.md` sections 16.3 and 19.2).

use crate::pipeline;
use klf_driver::{Backend, LanguageDefinition};
use klf_editor_gen::{Editor, Flag, Language, generate};
use klf_services::{DriverFlagValue, Session};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: klf editor gen <definition-package-dir> --out DIR [--editor NAMES] [--client FILE.js] [--timings]\n       klf editor gen --kernel --out DIR [--editor NAMES] [--client FILE.js] [--timings]\n\nNAMES is a comma list of vscode, zed, neovim, helix and emacs, or `all`.\nWithout --editor the VS Code and Zed packages are generated.";

pub fn command(args: &[String]) -> ExitCode {
    let options = match Gen::parse(args) {
        Ok(options) => options,
        Err(message) => return fail(&message),
    };
    let Gen {
        package,
        out,
        client,
        kernel,
        timings,
        mut editors,
    } = options;
    let mut finish = klf_diag::status::begin(
        &format!(
            "Generating editor packages for {}",
            package
                .as_ref()
                .map_or_else(|| "kernel".to_owned(), |path| path.display().to_string())
        ),
        timings,
    );
    if editors.is_empty() {
        editors = Editor::POPULAR.to_vec();
    }
    let languages = match (kernel, package) {
        (true, None) => kernel_languages(),
        (false, Some(package)) => match definition_language(&package) {
            Ok(language) => vec![language],
            Err(code) => return code,
        },
        _ => return fail(USAGE),
    };
    write_packages(&out, client, &languages, &editors, &mut finish)
}

/// What `klf editor gen` was asked for.
struct Gen {
    package: Option<PathBuf>,
    out: PathBuf,
    client: Option<PathBuf>,
    kernel: bool,
    timings: bool,
    editors: Vec<Editor>,
}

impl Gen {
    /// Reads the flags, or the message to refuse the command with.
    fn parse(args: &[String]) -> Result<Self, String> {
        if args.first().map(String::as_str) != Some("gen") {
            return Err(USAGE.to_owned());
        }
        let mut parsed = Self {
            package: None,
            out: PathBuf::new(),
            client: None,
            kernel: false,
            timings: false,
            editors: Vec::new(),
        };
        let mut rest = args[1..].iter();
        while let Some(argument) = rest.next() {
            match argument.as_str() {
                "--timings" => parsed.timings = true,
                "--out" => parsed.out = value(&mut rest)?.into(),
                "--client" => parsed.client = Some(value(&mut rest)?.into()),
                "--kernel" => parsed.kernel = true,
                "--editor" => {
                    add_editors(&mut parsed.editors, value(&mut rest)?)?;
                }
                flag if flag.starts_with('-') => {
                    return Err(format!("klf editor gen: unknown flag `{flag}`"));
                }
                path if parsed.package.is_none() => parsed.package = Some(path.into()),
                other => {
                    return Err(format!("klf editor gen: unexpected argument `{other}`"));
                }
            }
        }
        if parsed.out.as_os_str().is_empty() {
            return Err(USAGE.to_owned());
        }
        Ok(parsed)
    }
}

/// The value of the flag just read, or the usage line when it has none.
fn value<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<&'a str, String> {
    rest.next()
        .map(String::as_str)
        .ok_or_else(|| USAGE.to_owned())
}

/// Writes every editor package for `languages` under `out`.
///
/// The client script is read once, before the loop, because every package embeds
/// the same one and a caller that named it wants to know it is unreadable
/// before any package is half written.
fn write_packages(
    out: &Path,
    client: Option<PathBuf>,
    languages: &[Language],
    editors: &[Editor],
    finish: &mut klf_diag::status::Finish,
) -> ExitCode {
    let vscode = editors.contains(&Editor::VsCode);
    let client = match client.or_else(|| vscode.then(distributed_client).flatten()) {
        Some(path) => match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                return fail(&format!(
                    "klf editor gen: cannot read {}: {error}",
                    path.display()
                ));
            }
        },
        None => None,
    };
    let mut written = Vec::with_capacity(languages.len());
    for language in languages {
        klf_diag::progress::report(&format!("generate {}", language.name));
        let root = if languages.len() == 1 {
            out.to_path_buf()
        } else {
            out.join(&language.name)
        };
        match write_one(language, client.as_deref(), editors, &root) {
            Ok(count) => written.push((language.name.as_str(), count, root)),
            Err(message) => {
                finish.end();
                return fail(&message);
            }
        }
    }
    finish.end();
    for (name, count, root) in written {
        klf_diag::out!("Generated: Ok {name} ({count} files in {})", root.display());
    }
    if vscode && client.is_none() {
        klf_diag::err!(
            "note: no VS Code client script was found, so the .vsix highlights but starts no server; pass --client"
        );
    }
    ExitCode::SUCCESS
}

fn write_one(
    language: &Language,
    client: Option<&[u8]>,
    editors: &[Editor],
    root: &Path,
) -> Result<usize, String> {
    let files = generate(language, client, editors)
        .map_err(|error| format!("klf editor gen: {}: {error}", language.name))?;
    for file in &files {
        let path = root.join(&file.path);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, &file.contents));
        if let Err(error) = written {
            return Err(format!(
                "klf editor gen: cannot write {}: {error}",
                path.display()
            ));
        }
    }
    if editors.contains(&Editor::Zed)
        && language.spec.grammar.is_none()
        && let Err(message) = pin_zed_grammar(language, root)
    {
        return Err(message);
    }
    Ok(files.len())
}

/// Adds the editors of a comma list, each once.
fn add_editors(editors: &mut Vec<Editor>, names: &str) -> Result<(), String> {
    for name in names.split(',').map(str::trim) {
        let chosen = match name {
            "all" => Editor::ALL.to_vec(),
            _ => match Editor::from_name(name) {
                Some(editor) => vec![editor],
                None => {
                    return Err(format!(
                        "klf editor gen: unknown editor `{name}`; expected vscode, zed, neovim, helix, emacs or all"
                    ));
                }
            },
        };
        for editor in chosen {
            if !editors.contains(&editor) {
                editors.push(editor);
            }
        }
    }
    Ok(())
}

/// Points the Zed extension at the generated grammar, committed to git.
/// Only a failure to write is an error; without git the extension keeps
/// empty `repository` and `rev` and a warning says so.
fn pin_zed_grammar(language: &Language, root: &Path) -> Result<(), String> {
    match pin_grammar(&root.join("tree-sitter")) {
        Ok((repository, rev)) => {
            let file = klf_editor_gen::zed_extension(language, &repository, &rev);
            std::fs::write(root.join(&file.path), &file.contents).map_err(|error| {
                format!(
                    "klf editor gen: cannot write {}: {error}",
                    file.path.display()
                )
            })
        }
        Err(error) => {
            klf_diag::err!(
                "warning: {}: the Zed extension has no grammar repository: {error}",
                language.name
            );
            Ok(())
        }
    }
}

/// Makes the generated grammar a git repository and commits it, since Zed
/// fetches grammars from git by commit. Returns its `file://` URL and the
/// commit.
fn pin_grammar(directory: &Path) -> Result<(String, String), String> {
    let git = |arguments: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "user.name=klf", "-c", "user.email=klf@localhost"])
            .args(["-c", "commit.gpgsign=false"])
            .args(arguments)
            .output()
            .map_err(|error| format!("cannot run git: {error}"))
    };
    let checked = |output: std::process::Output| {
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    };
    if !directory.join(".git").exists() {
        checked(git(&["init", "-q"])?)?;
    }
    checked(git(&["add", "-A"])?)?;
    let unchanged = git(&["diff", "--cached", "--quiet"])?.status.success();
    let has_commit = git(&["rev-parse", "--verify", "-q", "HEAD"])?
        .status
        .success();
    if !unchanged || !has_commit {
        checked(git(&["commit", "-q", "-m", "Generated by klf editor gen"])?)?;
    }
    let rev = checked(git(&["rev-parse", "HEAD"])?)?;
    let absolute = std::fs::canonicalize(directory).map_err(|error| error.to_string())?;
    Ok((format!("file://{}", absolute.display()), rev))
}

/// The client script shipped in the KLF distribution, next to `bin/klf`.
fn distributed_client() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let path = executable
        .parent()?
        .parent()?
        .join("share/klf/editor/vscode-client.js");
    path.exists().then_some(path)
}

fn definition_language(package: &Path) -> Result<Language, ExitCode> {
    let target = klf_driver::host_triple()
        .parse()
        .map_err(|_| fail("klf editor gen: the host target is unknown"))?;
    let mut session = Session::new(target);
    let definition = LanguageDefinition::load(&mut session, package, Backend::Vm);
    if session.error_count() > 0 {
        klf_diag::raw_err!("{}", pipeline::render(&session));
        return Err(ExitCode::FAILURE);
    }
    let definition =
        definition.ok_or_else(|| fail("klf editor gen: definition package did not compile"))?;
    let descriptor = &definition.metadata().descriptor;
    Ok(Language {
        name: descriptor.name.clone(),
        driver: descriptor.driver.clone(),
        version: descriptor.version.clone(),
        extensions: descriptor.extensions.clone(),
        flags: descriptor
            .flags
            .iter()
            .map(|flag| Flag {
                name: flag.name.clone(),
                default: match &flag.default_value {
                    DriverFlagValue::Boolean(value) => serde_json::json!(value),
                    DriverFlagValue::Integer(value) => serde_json::json!(value),
                    DriverFlagValue::String(value) => serde_json::json!(value),
                },
            })
            .collect(),
        spec: descriptor.editor_spec(),
    })
}

/// The kernel file types, all served by `klf lsp`.
fn kernel_languages() -> Vec<Language> {
    let language = |name: &str, extensions: &[&str], spec| Language {
        name: name.to_owned(),
        driver: "klf".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        extensions: extensions
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect(),
        flags: Vec::new(),
        spec,
    };
    vec![
        language("klfd", &["klfd"], klf_kernel_ide::specs::klfd()),
        language("kir", &["kir"], klf_kernel_ide::specs::kir()),
        language("ridl", &["ridl", "ridlsig"], klf_kernel_ide::specs::ridl()),
    ]
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("{message}");
    ExitCode::FAILURE
}
