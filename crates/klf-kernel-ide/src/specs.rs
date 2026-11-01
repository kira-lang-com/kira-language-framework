//! The editor specs of the kernel file types `klf lsp` serves
//! (`KLF-EDITOR.md` section 16.3).

use crate::PACKAGE_MARKER;
use klf_facts::spec::{EditorSpec, KeywordGroup};
use klf_facts::{TokenClass, TokenModifier};
use klf_ide::Languages;

fn words(list: &str) -> Vec<String> {
    list.split_whitespace().map(str::to_owned).collect()
}

fn group(class: TokenClass, modifiers: Vec<TokenModifier>, list: &str) -> KeywordGroup {
    KeywordGroup {
        class,
        modifiers,
        words: words(list),
    }
}

/// KCore, the bootstrap language of definitions.
#[must_use]
pub fn kcore() -> EditorSpec {
    let mut spec = EditorSpec::minimal("kcore");
    "KCore".clone_into(&mut spec.display_name);
    spec.root_markers = vec!["language.kcore".to_owned(), PACKAGE_MARKER.to_owned()];
    spec.doc_comments = vec!["///".to_owned()];
    spec.annotation = Some("@[A-Za-z_][A-Za-z0-9_]*".to_owned());
    spec.keywords = vec![
        group(
            TokenClass::Keyword,
            vec![TokenModifier::Control],
            "if else while for in match attempt try return break continue",
        ),
        group(TokenClass::Boolean, Vec::new(), "true false"),
        group(
            TokenClass::Keyword,
            Vec::new(),
            "function let var import as is struct enum construct trait extends type distinct extend",
        ),
        group(
            TokenClass::Keyword,
            vec![TokenModifier::Contextual],
            "handle borrow mut move copy self",
        ),
    ];
    spec.operators = words("-> => == != <= >= && || + - * / % = < > ! . ? :");
    spec
}

/// The editor spec also used for legacy `.klfd` sources.
#[must_use]
pub fn klfd() -> EditorSpec {
    kcore()
}

/// Textual KIR.
#[must_use]
pub fn kir() -> EditorSpec {
    let mut spec = EditorSpec::minimal("kir");
    "KIR".clone_into(&mut spec.display_name);
    spec.block_comments = Vec::new();
    "[%@]?[A-Za-z_][A-Za-z0-9_.$]*".clone_into(&mut spec.identifier);
    spec.keywords = vec![
        group(
            TokenClass::Keyword,
            Vec::new(),
            "module lang target stage func type global trait vtable import export engine private public",
        ),
        group(
            TokenClass::Keyword,
            vec![TokenModifier::Control],
            "return br cond_br switch.int unreachable",
        ),
    ];
    spec
}

/// RIDL interfaces and their frozen signatures.
#[must_use]
pub fn ridl() -> EditorSpec {
    let mut spec = EditorSpec::minimal("ridl");
    "RIDL".clone_into(&mut spec.display_name);
    spec.annotation = Some("@[A-Za-z_][A-Za-z0-9_]*".to_owned());
    spec.keywords = vec![
        group(
            TokenClass::Keyword,
            Vec::new(),
            "module import struct enum trait function extend type let",
        ),
        group(
            TokenClass::Keyword,
            vec![TokenModifier::Contextual],
            "borrow mut self",
        ),
    ];
    spec
}

/// Every kernel file type, by extension.
#[must_use]
pub fn languages() -> Languages {
    Languages::single(kcore())
        .with("kcore", kcore())
        .with("klfd", kcore())
        .with("klfk", kcore())
        .with("kir", kir())
        .with("ridl", ridl())
        .with("ridlsig", ridl())
}
