//! The Zed extension source (`KLF-EDITOR.md` section 19.7).

use crate::treesitter::{Dialect, queries_for};
use crate::{File, Language};
use std::fmt::Write;

fn toml_string(text: &str) -> String {
    format!("{text:?}")
}

fn toml_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| toml_string(item)).collect();
    format!("[{}]", quoted.join(", "))
}

pub(crate) fn package(language: &Language) -> Result<Vec<File>, String> {
    let id = language.identifier();
    let server = format!("{id}-lsp");
    let mut files = vec![
        File::text("zed/extension.toml", extension_toml(language, None)),
        File::text(
            "zed/Cargo.toml",
            format!(
                "[package]\nname = {}\nversion = {}\nedition = \"2021\"\npublish = false\n\n\
                 [lib]\npath = \"src/lib.rs\"\ncrate-type = [\"cdylib\"]\n\n\
                 [dependencies]\nzed_extension_api = \"0.7.0\"\n",
                toml_string(&format!("zed-{}", id.replace('_', "-"))),
                toml_string(&language.version)
            ),
        ),
        File::text("zed/src/lib.rs", extension_source(language, &server)),
        File::text(
            format!("zed/languages/{id}/config.toml"),
            config_toml(language, &id),
        ),
    ];
    for (name, text) in queries_for(language, Dialect::Zed)? {
        if matches!(
            name,
            "highlights.scm" | "brackets.scm" | "indents.scm" | "outline.scm"
        ) {
            files.push(File::text(format!("zed/languages/{id}/{name}"), text));
        }
    }
    Ok(files)
}

/// `extension.toml`. `lexical` is the git repository and commit of the
/// generated grammar, which Zed fetches with `git fetch <rev>`; without it
/// the file says how to provide them.
pub(crate) fn extension_toml(language: &Language, lexical: Option<(&str, &str)>) -> String {
    let spec = &language.spec;
    let id = language.identifier();
    let server = format!("{id}-lsp");
    let mut extension = format!(
        "id = {}\nname = {}\nversion = {}\nschema_version = 1\nauthors = []\ndescription = {}\n\n\
         [language_servers.{server}]\nname = {}\nlanguages = [{}]\n\n[grammars.{id}]\n",
        toml_string(&id.replace('_', "-")),
        toml_string(&spec.display_name),
        toml_string(&language.version),
        toml_string(&format!(
            "{} support, generated from its KLF definition",
            spec.display_name
        )),
        toml_string(&format!("{} language server", spec.display_name)),
        toml_string(&spec.display_name),
    );
    match (&spec.grammar, lexical) {
        (Some(grammar), _) => {
            let _ = writeln!(
                extension,
                "repository = {}\nrev = {}\npath = {}",
                toml_string(&grammar.repository),
                toml_string(&grammar.revision),
                toml_string(&grammar.path)
            );
        }
        (None, Some((repository, rev))) => {
            let _ = writeln!(
                extension,
                "repository = {}\nrev = {}",
                toml_string(repository),
                toml_string(rev)
            );
        }
        (None, None) => {
            extension.push_str(
                "# Zed fetches grammars from git by commit. `klf editor gen` makes\n\
                 # `../tree-sitter` a repository and fills these in.\n\
                 repository = \"\"\nrev = \"\"\n",
            );
        }
    }
    extension
}

fn config_toml(language: &Language, id: &str) -> String {
    let spec = &language.spec;
    let mut config = format!(
        "name = {}\ngrammar = {}\npath_suffixes = {}\n",
        toml_string(&spec.display_name),
        toml_string(id),
        toml_list(&language.extensions)
    );
    if !spec.line_comments.is_empty() {
        let prefixes: Vec<String> = spec
            .line_comments
            .iter()
            .map(|prefix| format!("{prefix} "))
            .collect();
        let _ = writeln!(config, "line_comments = {}", toml_list(&prefixes));
    }
    if let Some(block) = spec.block_comments.first() {
        let _ = writeln!(
            config,
            "block_comment = [{}, {}]",
            toml_string(&format!("{} ", block.open)),
            toml_string(&format!(" {}", block.close))
        );
    }
    if spec.indent.unit == "\t" {
        config.push_str("hard_tabs = true\n");
    } else {
        let _ = writeln!(
            config,
            "tab_size = {}",
            spec.indent.unit.chars().count().max(1)
        );
    }
    let closers: String = spec
        .brackets
        .iter()
        .map(|pair| pair.close.as_str())
        .collect();
    let _ = writeln!(
        config,
        "autoclose_before = {}",
        toml_string(&format!(";:.,={closers}"))
    );
    config.push_str("brackets = [\n");
    for pair in &spec.auto_close {
        let newline = spec
            .brackets
            .iter()
            .any(|bracket| bracket.open == pair.open);
        let _ = writeln!(
            config,
            "  {{ start = {}, end = {}, close = true, newline = {newline} }},",
            toml_string(&pair.open),
            toml_string(&pair.close)
        );
    }
    config.push_str("]\n");
    config
}

fn extension_source(language: &Language, server: &str) -> String {
    EXTENSION
        .replace("@SERVER@", server)
        .replace("@DRIVER@", &language.driver)
        .replace("@SYSROOT@", &language.sysroot_variable())
        .replace("@NAME@", &language.spec.display_name)
}

const EXTENSION: &str = r#"//! Generated by klf editor gen. Finds the language server in the order of
//! KLF-EDITOR.md section 19.6: the `binary` setting, the sysroot named by
//! `@SYSROOT@`, then `@DRIVER@` on the worktree's PATH.

use zed_extension_api::settings::LspSettings;
use zed_extension_api::{self as zed, LanguageServerId, Result, serde_json};

struct Klf;

impl zed::Extension for Klf {
    fn new() -> Self {
        Klf
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree("@SERVER@", worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let arguments = binary
            .as_ref()
            .and_then(|binary| binary.arguments.clone())
            .unwrap_or_else(|| vec!["lsp".to_owned()]);
        let env = worktree.shell_env();
        let command = if let Some(path) = binary.and_then(|binary| binary.path) {
            path
        } else if let Some((_, sysroot)) = env.iter().find(|(name, _)| name == "@SYSROOT@") {
            format!("{sysroot}/bin/@DRIVER@")
        } else if let Some(path) = worktree.which("@DRIVER@") {
            path
        } else {
            return Err(
                "@NAME@: no language server. Set lsp.@SERVER@.binary.path, set @SYSROOT@, or put @DRIVER@ on PATH"
                    .to_owned(),
            );
        };
        Ok(zed::Command {
            command,
            args: arguments,
            env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        Ok(LspSettings::for_worktree("@SERVER@", worktree)
            .ok()
            .and_then(|settings| settings.initialization_options))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        Ok(LspSettings::for_worktree("@SERVER@", worktree)
            .ok()
            .and_then(|settings| settings.settings))
    }
}

zed::register_extension!(Klf);
"#;
