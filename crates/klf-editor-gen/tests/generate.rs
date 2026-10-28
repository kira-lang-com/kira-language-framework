//! Generated packages: valid JSON, a vsix that unzips, and a tree-sitter
//! grammar that compiles, loads and parses with its queries.

use klf_editor_gen::{Editor, Flag, Language, generate};
use klf_facts::spec::{BlockComment, EditorSpec, KeywordGroup, StringForm};
use klf_facts::{TokenClass, TokenModifier};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn toy() -> Language {
    let mut spec = EditorSpec::minimal("toy");
    spec.block_comments = vec![BlockComment {
        open: "/*".to_owned(),
        close: "*/".to_owned(),
        nests: true,
    }];
    spec.doc_comments = vec!["///".to_owned()];
    spec.strings.push(StringForm {
        open: "'".to_owned(),
        close: "'".to_owned(),
        escape: Some("\\\\.".to_owned()),
        multiline: false,
        raw: false,
        interpolation: None,
        class: TokenClass::Character,
    });
    spec.annotation = Some("@[A-Za-z_][A-Za-z0-9_]*".to_owned());
    spec.keywords = vec![
        KeywordGroup {
            class: TokenClass::Keyword,
            modifiers: vec![TokenModifier::Control],
            words: vec!["if".to_owned(), "else".to_owned(), "return".to_owned()],
        },
        KeywordGroup {
            class: TokenClass::Keyword,
            modifiers: Vec::new(),
            words: vec!["function".to_owned(), "let".to_owned()],
        },
        KeywordGroup {
            class: TokenClass::Boolean,
            modifiers: Vec::new(),
            words: vec!["true".to_owned(), "false".to_owned()],
        },
        KeywordGroup {
            class: TokenClass::Keyword,
            modifiers: vec![TokenModifier::Contextual],
            words: vec!["borrow".to_owned()],
        },
    ];
    spec.operators = ["->", "==", "=", "+", "-", "*", "/", "<", ">", ".", ","]
        .map(str::to_owned)
        .to_vec();
    Language {
        name: "toy".to_owned(),
        driver: "toyc".to_owned(),
        version: "0.3.1".to_owned(),
        extensions: vec!["toy".to_owned()],
        flags: vec![Flag {
            name: "strict".to_owned(),
            default: json!(false),
        }],
        spec,
    }
}

fn files(language: &Language) -> HashMap<PathBuf, Vec<u8>> {
    generate(language, None, &Editor::ALL)
        .expect("generates")
        .into_iter()
        .map(|file| (file.path, file.contents))
        .collect()
}

fn json_of(files: &HashMap<PathBuf, Vec<u8>>, path: &str) -> Value {
    serde_json::from_slice(&files[Path::new(path)])
        .unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn the_tree_holds_every_package_of_section_19() {
    let files = files(&toy());
    for path in [
        "manifest.json",
        "textmate/toy.tmLanguage.json",
        "language-configuration.json",
        "tree-sitter/src/grammar.json",
        "tree-sitter/src/parser.c",
        "tree-sitter/src/scanner.c",
        "tree-sitter/src/tree_sitter/parser.h",
        "tree-sitter/queries/highlights.scm",
        "vscode/toy-0.3.1.vsix",
        "zed/extension.toml",
        "zed/src/lib.rs",
        "zed/languages/toy/config.toml",
        "neovim/toy.lua",
        "helix/languages.toml",
        "helix/queries/toy/highlights.scm",
        "emacs/toy-mode.el",
    ] {
        assert!(files.contains_key(Path::new(path)), "{path} is generated");
    }
    let manifest = json_of(&files, "manifest.json");
    assert_eq!(manifest["driver"], "toyc");
    assert_eq!(manifest["server"]["environment"], "KLF_TOY_SYSROOT");
    let grammar = json_of(&files, "textmate/toy.tmLanguage.json");
    assert_eq!(grammar["scopeName"], "source.toy");
    assert_eq!(
        grammar["repository"]["keywords_0"]["match"],
        "\\b(?:return|else|if)\\b"
    );
    assert_eq!(
        grammar["repository"]["keywords_0"]["name"],
        "keyword.control.toy"
    );
    let configuration = json_of(&files, "language-configuration.json");
    assert_eq!(configuration["comments"]["lineComment"], "//");
    assert_eq!(configuration["blockComment"], Value::Null);
    assert_eq!(
        configuration["comments"]["blockComment"],
        json!(["/*", "*/"])
    );
}

#[test]
fn the_vsix_is_a_zip_with_the_extension_manifest() {
    let files = files(&toy());
    let directory =
        std::env::temp_dir().join(format!("klf-editor-gen-vsix-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("directory");
    let vsix = directory.join("toy.vsix");
    std::fs::write(&vsix, &files[Path::new("vscode/toy-0.3.1.vsix")]).expect("written");
    let Ok(listing) = Command::new("unzip").arg("-t").arg(&vsix).output() else {
        eprintln!("skipping: no unzip");
        return;
    };
    let text = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.status.success(), "{text}");
    for entry in [
        "extension.vsixmanifest",
        "[Content_Types].xml",
        "extension/package.json",
    ] {
        assert!(text.contains(entry), "{entry} in {text}");
    }
    let package = Command::new("unzip")
        .arg("-p")
        .arg(&vsix)
        .arg("extension/package.json")
        .output()
        .expect("unzip runs");
    let package: Value = serde_json::from_slice(&package.stdout).expect("package.json parses");
    assert_eq!(
        package["contributes"]["languages"][0]["extensions"],
        json!([".toy"])
    );
    assert_eq!(
        package["contributes"]["configuration"]["properties"]["toy.flags.strict"]["type"],
        "boolean"
    );
    assert_eq!(package["main"], Value::Null, "no client, no server start");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_vsix_with_the_client_starts_it_and_offers_the_protocol_commands() {
    let client = b"exports.activate = () => {};";
    let vsix = generate(&toy(), Some(client), &[Editor::VsCode])
        .expect("generates")
        .into_iter()
        .find(|file| file.path == Path::new("vscode/toy-0.3.1.vsix"))
        .expect("a vsix");
    let directory =
        std::env::temp_dir().join(format!("klf-editor-gen-client-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("toy.vsix");
    std::fs::write(&path, &vsix.contents).expect("written");
    let entry = |name: &str| {
        Command::new("unzip")
            .arg("-p")
            .arg(&path)
            .arg(name)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| output.stdout)
    };
    let Some(package) = entry("extension/package.json") else {
        eprintln!("skipping: no unzip");
        return;
    };
    assert_eq!(
        entry("extension/extension.js").as_deref(),
        Some(&client[..])
    );
    let package: Value = serde_json::from_slice(&package).expect("package.json parses");
    assert_eq!(package["main"], "./extension.js");
    let commands: Vec<&str> = package["contributes"]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter_map(|command| command["command"].as_str())
        .collect();
    for command in [
        "klf.run",
        "klf.reloadProjects",
        "klf.showOutput",
        "klf.showKir",
        "klf.showExpansion",
        "klf.restartServer",
    ] {
        assert!(commands.contains(&command), "{command} in {commands:?}");
    }
    let manifest: Value =
        serde_json::from_slice(&entry("extension/manifest.json").expect("manifest"))
            .expect("manifest parses");
    assert_eq!(manifest["server"]["setting"], "toy.server.path");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_tree_sitter_grammar_parses_and_its_queries_compile() {
    let files = files(&toy());
    let directory = std::env::temp_dir().join(format!("klf-editor-gen-ts-{}", std::process::id()));
    for (path, contents) in &files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().expect("parent")).expect("directory");
        std::fs::write(&target, contents).expect("written");
    }
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("skipping: no C compiler");
        return;
    }
    let loader = tree_sitter_loader::Loader::with_parser_lib_path(directory.join("lib"));
    let source = directory.join("tree-sitter/src");
    let language = loader
        .load_language_at_path(tree_sitter_loader::CompileConfig::new(&source, None, None))
        .unwrap_or_else(|error| panic!("the generated parser compiles and loads: {error}"));
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).expect("compatible ABI");
    let text = "/// Adds.\nfunction add(a, b) {\n    /* outer /* inner */ still */\n    let s = \"a \\\" b\" + 'c'\n    if borrow == true { return add(a, -1) } // done\n}\n@Main\n";
    let tree = parser.parse(text, None).expect("parses");
    let root = tree.root_node();
    let sexp = root.to_sexp();
    assert!(!root.has_error(), "{sexp}");
    for node in [
        "(doc_comment)",
        "(comment)",
        "(string)",
        "(character)",
        "(keyword_0)",
        "(keyword_2)",
        "(annotation)",
        "(bracket_0",
    ] {
        assert!(sexp.contains(node), "{node} in {sexp}");
    }
    let comment = root
        .descendant_for_byte_range(
            text.find("/* outer").expect("comment"),
            text.find("/* outer").expect("comment") + 1,
        )
        .expect("node");
    assert_eq!(
        comment.utf8_text(text.as_bytes()).expect("utf-8"),
        "/* outer /* inner */ still */",
        "block comments nest"
    );

    for directory in [
        directory.join("tree-sitter/queries"),
        directory.join("zed/languages/toy"),
        directory.join("helix/queries/toy"),
    ] {
        for entry in std::fs::read_dir(&directory).expect("queries") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|extension| extension == "scm") {
                let query = std::fs::read_to_string(&path).expect("query");
                if let Err(error) = tree_sitter::Query::new(&language, &query) {
                    panic!("{}: {error}", path.display());
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_zed_extension_uses_zed_capture_names_and_a_pinned_grammar() {
    let language = toy();
    let files = files(&language);
    let text = |path: &str| String::from_utf8(files[Path::new(path)].clone()).expect("utf-8");
    let highlights = text("zed/languages/toy/highlights.scm");
    assert!(highlights.contains("@comment.doc"));
    assert!(!highlights.contains("@comment.documentation"));
    assert!(!highlights.contains("@character"));
    assert!(text("zed/extension.toml").contains("rev = \"\""));
    let pinned = klf_editor_gen::zed_extension(&language, "file:///grammar", "abc123");
    let pinned = String::from_utf8(pinned.contents).expect("utf-8");
    assert!(pinned.contains("repository = \"file:///grammar\"\nrev = \"abc123\""));
}

#[test]
fn only_the_chosen_editors_are_generated() {
    let chosen = |editors: &[Editor]| -> Vec<PathBuf> {
        generate(&toy(), None, editors)
            .expect("generates")
            .into_iter()
            .map(|file| file.path)
            .collect()
    };
    let vscode = chosen(&[Editor::VsCode]);
    assert!(vscode.contains(&PathBuf::from("vscode/toy-0.3.1.vsix")));
    assert!(
        vscode
            .iter()
            .all(|path| !path.starts_with("tree-sitter") && !path.starts_with("zed"))
    );
    let zed = chosen(&[Editor::Zed]);
    assert!(zed.contains(&PathBuf::from("zed/extension.toml")));
    assert!(zed.contains(&PathBuf::from("tree-sitter/src/parser.c")));
    assert!(
        zed.iter()
            .all(|path| !path.starts_with("vscode") && !path.starts_with("helix"))
    );
    assert_eq!(Editor::from_name("neovim"), Some(Editor::Neovim));
}
