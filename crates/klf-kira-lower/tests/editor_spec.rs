//! The `editor` member of a `Language` declaration (KLF-EDITOR section 5.2).

use klf_core::FileId;
use klf_facts::spec::{AutoCloseContext, EditorSpec};
use klf_facts::{TokenClass, TokenModifier};
use klf_kira_lower::extract_language_descriptor;
use klf_kira_syntax::parse;
use klf_services::{LanguageDescriptor, Session};
use klf_source::SourceFile;

const HEADER: &str = r#"construct Toy() extends Language<Tree, Program> {
    let name: String = "toy"
    let driver: String = "toy"
    let version: String = "0.1.0"
    let extensions: [String] = ["toy"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Platform
    let stdRoot: String = ""
"#;

fn descriptor(editor: &str) -> (Option<LanguageDescriptor>, Vec<String>) {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let source = format!("{HEADER}{editor}}}\n");
    let file = SourceFile::new(FileId::from_u32(0), "toy.klfd", source.as_str());
    let parsed = parse(&file);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let descriptor = extract_language_descriptor(&mut session, &parsed.file);
    let codes = session
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    (descriptor, codes)
}

const EDITOR: &str = r##"    let editor: EditorSpec = EditorSpec {
        displayName = "Toy", scope = "source.toy", rootMarkers = ["toy.toml"],
        lineComments = ["#"],
        blockComments = [BlockComment { open = "(*", close = "*)", nests = true }],
        strings = [StringForm { open = "\"", close = "\"", escape = .Some("\\\\."), multiline = false,
            raw = false, interpolation = .Some(BracketPair { open = "${", close = "}" }), class = .String }],
        numbers = ["\\d+"], identifier = "[a-z]+",
        keywords = [
            KeywordGroup { class = .Keyword, modifiers = [.Control], words = ["if", "else"] },
            KeywordGroup { class = .Keyword, modifiers = [.Contextual], words = ["of"] }
        ],
        operators = ["+", "->"],
        brackets = [BracketPair { open = "(", close = ")" }],
        autoClose = [AutoClosePair { open = "(", close = ")", notIn = [.String, .Comment] }],
        indent = IndentRules { unit = "  ", increase = .Some("\\($"), decrease = .None },
        onEnter = [OnEnterRule { before = "^\\s*#", after = .None, action = .None, append = "# " }]
    }
"##;

#[test]
fn an_editor_member_is_read_into_the_descriptor() {
    let (descriptor, codes) = descriptor(EDITOR);
    let descriptor = descriptor.unwrap_or_else(|| panic!("descriptor fails: {codes:?}"));
    let spec = descriptor.editor.clone().expect("editor declared");
    assert_eq!(spec.display_name, "Toy");
    assert_eq!(spec.root_markers, ["toy.toml"]);
    assert!(spec.block_comments[0].nests);
    assert_eq!(spec.strings[0].escape.as_deref(), Some("\\\\."));
    assert_eq!(
        spec.strings[0]
            .interpolation
            .as_ref()
            .map(|pair| pair.open.as_str()),
        Some("${")
    );
    assert_eq!(spec.keywords[0].modifiers, [TokenModifier::Control]);
    assert_eq!(spec.keywords[1].class, TokenClass::Keyword);
    assert_eq!(
        spec.auto_close[0].not_in,
        [AutoCloseContext::String, AutoCloseContext::Comment]
    );
    assert_eq!(spec.indent.decrease, None);
    assert_eq!(spec.on_enter[0].append, "# ");
    assert_eq!(EditorSpec::decode(&spec.encode()), Ok(spec));
}

#[test]
fn a_language_without_an_editor_member_gets_the_minimal_spec() {
    let (descriptor, _) = descriptor("");
    let descriptor = descriptor.expect("descriptor");
    assert_eq!(descriptor.editor, None);
    assert_eq!(descriptor.editor_spec(), EditorSpec::minimal("toy"));
}

#[test]
fn a_pattern_outside_the_dialect_is_rejected() {
    let (descriptor, codes) =
        descriptor(&EDITOR.replace(r#"identifier = "[a-z]+""#, r#"identifier = "(?=a)""#));
    assert!(descriptor.is_none());
    assert_eq!(codes, ["KLF-DEF-0040"]);
}
