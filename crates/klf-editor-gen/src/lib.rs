//! Editor packages generated from a definition's editor spec
//! (`KLF-EDITOR.md` section 19): nothing in them is written by hand per
//! language.

mod configuration;
mod emacs;
mod helix;
mod neovim;
mod regex;
mod scanner;
mod scopes;
mod textmate;
mod treesitter;
mod vscode;
mod zed;
mod zip;

use klf_facts::spec::EditorSpec;
use serde_json::{Value, json};
use std::path::PathBuf;

/// The protocol version of the `klf/` extensions the packages expect
/// (section 19.9).
pub const PROTOCOL_VERSION: u32 = 1;

/// What a package is generated from: the definition's `name`, `driver`,
/// `version`, `extensions`, `flags` and `editor`.
#[derive(Clone, Debug)]
pub struct Language {
    pub name: String,
    pub driver: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub flags: Vec<Flag>,
    pub spec: EditorSpec,
}

/// A driver flag, which packages expose as the setting
/// `<lang>.flags.<name>` (Appendix B).
#[derive(Clone, Debug)]
pub struct Flag {
    pub name: String,
    pub default: Value,
}

/// One file of a generated tree, relative to its root.
#[derive(Clone, Debug)]
pub struct File {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

impl File {
    fn text(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into().into_bytes(),
        }
    }

    fn json(path: impl Into<PathBuf>, value: &Value) -> Self {
        let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
        text.push('\n');
        Self::text(path, text)
    }
}

impl Language {
    /// The language name as an identifier: tree-sitter, Emacs and Lua need
    /// one.
    #[must_use]
    pub fn identifier(&self) -> String {
        self.name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// The environment variable naming the sysroot (section 19.6).
    #[must_use]
    pub fn sysroot_variable(&self) -> String {
        format!("KLF_{}_SYSROOT", self.identifier().to_ascii_uppercase())
    }

    /// The default value of each setting of Appendix B a package declares.
    fn settings(&self) -> Vec<(String, Value, &'static str)> {
        let lang = &self.name;
        let mut settings = vec![
            (
                format!("{lang}.server.path"),
                json!(""),
                "Path of the language server; empty to search the sysroot and PATH",
            ),
            (
                format!("{lang}.server.arguments"),
                json!([]),
                "Arguments passed to the language server after `lsp`",
            ),
        ];
        for flag in &self.flags {
            settings.push((
                format!("{lang}.flags.{}", flag.name),
                flag.default.clone(),
                "Driver flag appended to every project's arguments",
            ));
        }
        let kernel: [(&str, Value, &'static str); 10] = [
            (
                "klf.diagnostics.debounceMs",
                json!(50),
                "Delay before analyzing after an edit",
            ),
            (
                "klf.diagnostics.workspace",
                json!(false),
                "Publish diagnostics for files that are not open",
            ),
            (
                "klf.completion.limit",
                json!(200),
                "Maximum completion items",
            ),
            (
                "klf.workspaceSymbols.limit",
                json!(256),
                "Maximum workspace symbols",
            ),
            ("klf.inlayHints.types", json!(true), "Show type hints"),
            (
                "klf.inlayHints.parameters",
                json!(true),
                "Show parameter hints",
            ),
            (
                "klf.lens.references",
                json!(false),
                "Show reference counts above declarations",
            ),
            (
                "klf.lens.runnables",
                json!(true),
                "Show run and test lenses",
            ),
            ("klf.index", json!(true), "Index the workspace"),
            (
                "klf.trace.server",
                json!("off"),
                "Trace the protocol: off, messages or verbose",
            ),
        ];
        settings.extend(
            kernel
                .into_iter()
                .map(|(name, value, about)| (name.to_owned(), value, about)),
        );
        settings
    }
}

/// An editor a package is generated for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    VsCode,
    Zed,
    Neovim,
    Helix,
    Emacs,
}

impl Editor {
    pub const ALL: [Self; 5] = [
        Self::VsCode,
        Self::Zed,
        Self::Neovim,
        Self::Helix,
        Self::Emacs,
    ];

    /// The editors generated when none are named.
    pub const POPULAR: [Self; 2] = [Self::VsCode, Self::Zed];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::VsCode => "vscode",
            Self::Zed => "zed",
            Self::Neovim => "neovim",
            Self::Helix => "helix",
            Self::Emacs => "emacs",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|editor| editor.name() == name)
    }

    /// Whether the package uses the generated tree-sitter grammar.
    fn uses_tree_sitter(self) -> bool {
        self != Self::VsCode
    }
}

/// Generates the tree of section 19.1 with the packages of `editors`.
/// `client` is the VS Code client script of the KLF distribution; without
/// it the `.vsix` holds the grammar and configuration but starts no server.
///
/// # Errors
/// Returns the first spec problem or generator failure.
pub fn generate(
    language: &Language,
    client: Option<&[u8]>,
    editors: &[Editor],
) -> Result<Vec<File>, String> {
    let problems = language.spec.validate();
    if let Some((code, message)) = problems.first() {
        return Err(format!("{code}: {message}"));
    }
    let id = language.identifier();
    let grammar = textmate::grammar(language)?;
    let configuration = configuration::configuration(language)?;
    let mut files = vec![
        File::json("manifest.json", &manifest(language)),
        File::json(format!("textmate/{id}.tmLanguage.json"), &grammar),
        File::json("language-configuration.json", &configuration),
    ];
    if editors.iter().any(|editor| editor.uses_tree_sitter()) {
        let tree_sitter = treesitter::generate(language)?;
        files.extend(tree_sitter.into_iter().map(|file| File {
            path: PathBuf::from("tree-sitter").join(file.path),
            contents: file.contents,
        }));
    }
    for editor in editors {
        match editor {
            Editor::VsCode => {
                files.push(vscode::package(language, &grammar, &configuration, client)?);
            }
            Editor::Zed => files.extend(zed::package(language)?),
            Editor::Neovim => files.push(neovim::package(language)),
            Editor::Helix => files.extend(helix::package(language)),
            Editor::Emacs => files.push(emacs::package(language)?),
        }
    }
    Ok(files)
}

/// The Zed `extension.toml` with the generated grammar fetched from
/// `repository` at commit `rev`.
#[must_use]
pub fn zed_extension(language: &Language, repository: &str, rev: &str) -> File {
    File::text(
        "zed/extension.toml",
        zed::extension_toml(language, Some((repository, rev))),
    )
}

fn manifest(language: &Language) -> Value {
    let (types, modifiers) = klf_lsp::legend_names();
    json!({
        "language": language.name,
        "displayName": language.spec.display_name,
        "driver": language.driver,
        "version": language.version,
        "extensions": language.extensions,
        "fileNames": language.spec.file_names,
        "scope": language.spec.scope,
        "protocol": PROTOCOL_VERSION,
        "server": {
            "arguments": ["lsp"],
            "setting": format!("{}.server.path", language.name),
            "environment": language.sysroot_variable(),
        },
        "legend": { "tokenTypes": types, "tokenModifiers": modifiers },
    })
}
