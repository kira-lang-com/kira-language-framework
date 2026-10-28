use crate::lexical::Classifier;
use crate::pattern::{Engine, Pattern};
use crate::spec::{EditorSpec, KeywordGroup};
use crate::*;
use klf_core::FileId;

const FILE: FileId = FileId::from_u32(0);

#[test]
fn later_phases_and_inner_spans_win() {
    let mut facts = Facts::new();
    facts.set_phase(FactPhase::Parse);
    facts.classify(FILE, 0, 10, TokenClass::String, 0);
    facts.classify(FILE, 12, 15, TokenClass::Variable, 0);
    facts.classify(FILE, 3, 5, TokenClass::Escape, 0);
    facts.set_phase(FactPhase::Elaborate);
    facts.classify(
        FILE,
        12,
        15,
        TokenClass::Function,
        TokenModifier::Declaration.bit(),
    );
    let tokens: Vec<(u32, u32, TokenClass)> = facts
        .tokens(FILE)
        .iter()
        .map(|token| (token.start, token.end, token.class))
        .collect();
    assert_eq!(
        tokens,
        vec![
            (0, 3, TokenClass::String),
            (3, 5, TokenClass::Escape),
            (5, 10, TokenClass::String),
            (12, 15, TokenClass::Function),
        ]
    );
}

fn symbol(name: &str, start: u32, end: u32, container: Option<Entity>) -> Symbol {
    Symbol {
        name: name.to_owned(),
        kind: Some(SymbolKind::Function),
        file: Some(FILE),
        start,
        end,
        selection_start: start + 9,
        selection_end: start + 9 + u32::try_from(name.len()).unwrap(),
        container,
        ..Symbol::default()
    }
}

#[test]
fn navigation_reads_symbols_and_references() {
    let mut facts = Facts::new();
    let outer = Entity(1);
    let inner = Entity(2);
    facts.declare_symbol(outer, symbol("outer", 0, 100, None));
    facts.declare_symbol(inner, symbol("inner", 20, 60, Some(outer)));
    facts.reference(FILE, 70, 75, inner, ReferenceRole::Call);
    facts.reference(FILE, 80, 85, inner, ReferenceRole::Read);

    assert_eq!(
        facts.entity_at(FILE, 72).map(|(_, entity)| entity),
        Some(inner)
    );
    assert_eq!(
        facts.entity_at(FILE, 75).map(|(_, entity)| entity),
        Some(inner)
    );
    assert_eq!(
        facts.entity_at(FILE, 11).map(|(_, entity)| entity),
        Some(outer)
    );
    assert_eq!(facts.entity_at(FILE, 50), None);
    let definition = facts.definition(inner).unwrap();
    assert_eq!((definition.start, definition.end), (29, 34));
    assert_eq!(facts.references_to(inner).len(), 2);
    let outline = facts.document_symbols(FILE);
    assert_eq!(outline.len(), 1);
    assert_eq!(outline[0].entity, outer);
    assert_eq!(outline[0].children[0].entity, inner);
    assert_eq!(facts.workspace_symbols("inr", 10), vec![inner]);
    assert_eq!(facts.workspace_symbols("", 10).len(), 2);

    facts.classify(
        FILE,
        0,
        5,
        TokenClass::Keyword,
        TokenModifier::Control.bit(),
    );
    facts.type_at(FILE, 70, 75, "fn\t(Int)".to_owned(), Some(outer));
    facts.hint(
        FILE,
        Hint {
            at: 75,
            label: ": Int".to_owned(),
            kind: HintKind::Type,
            padding_left: true,
            padding_right: false,
            target: None,
            insert: Some(": Int".to_owned()),
        },
    );
    facts.link(
        FILE,
        0,
        2,
        LinkTarget::Url("https://example.org".to_owned()),
    );
    facts.runnable(
        FILE,
        Runnable {
            label: "run\nmain".to_owned(),
            kind: RunnableKind::Main,
            start: 0,
            end: 100,
            arguments: vec!["run".to_owned(), String::new()],
        },
    );
    facts.scope(FILE, 20, 60, 9);
    facts.call_edge(CallEdge {
        caller: outer,
        callee: inner,
        file: FILE,
        start: 70,
        end: 75,
    });
    facts.type_edge(inner, outer);
    facts.symbol_mut(inner).unwrap().parameters = vec!["a".to_owned(), "b".to_owned()];
    assert_eq!(Facts::decode(&facts.encode()), Ok(facts));
}

#[test]
fn absorbing_keeps_later_facts_later() {
    let mut first = Facts::new();
    first.classify(FILE, 0, 3, TokenClass::Variable, 0);
    let mut second = Facts::new();
    second.classify(FILE, 0, 3, TokenClass::Parameter, 0);
    first.absorb(second);
    assert_eq!(first.tokens(FILE)[0].class, TokenClass::Parameter);
}

#[test]
fn patterns_parse_translate_and_match() {
    let pattern = Pattern::parse("[A-Za-z_][A-Za-z0-9_]*").unwrap();
    assert_eq!(pattern.match_at("abc1 x", 0), Some(4));
    assert_eq!(pattern.match_at("1abc", 0), None);
    let alternation = Pattern::parse("(?:if|import)\\b").unwrap();
    assert_eq!(alternation.match_at("import x", 0), Some(6));
    assert_eq!(alternation.match_at("imports", 0), None);
    let number = Pattern::parse("\\d+(?:\\.\\d+)?").unwrap();
    assert_eq!(number.match_at("3.25)", 0), Some(4));
    assert_eq!(number.match_at("3.x", 0), Some(1));
    assert!(
        Pattern::parse("\\\\[ntr0\"\\\\]")
            .unwrap()
            .matches_whole("\\n")
    );
    assert_eq!(
        Pattern::parse("\\d{2,3}").unwrap().to_engine(Engine::Rust),
        "[0-9]{2,3}"
    );
    assert_eq!(
        Pattern::parse("(?:a|b)\\b")
            .unwrap()
            .to_engine(Engine::Emacs),
        "\\(?:a\\|b\\)\\b"
    );
    assert_eq!(
        Pattern::parse("a/b").unwrap().to_engine(Engine::JavaScript),
        "a\\/b"
    );
    assert_eq!(
        Pattern::parse("x(?:ab)*").unwrap().find("zzxababy", 0),
        Some((2, 7))
    );
}

#[test]
fn patterns_outside_the_dialect_are_refused() {
    for source in [
        "(a)", "a*?", "(?=a)", "\\1", "\\p{L}", "[z-a]", "*a", "a{3,2}",
    ] {
        assert!(
            Pattern::parse(source).is_err(),
            "{source} should be refused"
        );
    }
}

#[test]
fn lexical_classification_follows_the_spec() {
    let mut spec = EditorSpec::minimal("toy");
    spec.keywords.push(KeywordGroup {
        class: TokenClass::Keyword,
        modifiers: vec![TokenModifier::Control],
        words: vec!["if".to_owned(), "return".to_owned()],
    });
    spec.block_comments[0].nests = true;
    spec.operators = vec!["+".to_owned(), "==".to_owned()];
    let classifier = Classifier::new(&spec).unwrap();
    let text = "if x == 12 /* a /* b */ c */ return \"a\\nb\" // done";
    let classes: Vec<(&str, TokenClass)> = classifier
        .classify(text)
        .iter()
        .map(|token| (&text[token.start as usize..token.end as usize], token.class))
        .collect();
    assert_eq!(
        classes,
        vec![
            ("if", TokenClass::Keyword),
            ("==", TokenClass::Operator),
            ("12", TokenClass::Number),
            ("/* a /* b */ c */", TokenClass::Comment),
            ("return", TokenClass::Keyword),
            ("\"a", TokenClass::String),
            ("\\n", TokenClass::Escape),
            ("b\"", TokenClass::String),
            ("// done", TokenClass::Comment),
        ]
    );
}

#[test]
fn specs_are_validated() {
    let mut spec = EditorSpec::minimal("toy");
    assert_eq!(spec.validate(), Vec::<(&str, String)>::new());
    spec.identifier = "(x)".to_owned();
    spec.keywords.push(KeywordGroup {
        class: TokenClass::Keyword,
        modifiers: Vec::new(),
        words: vec!["a".to_owned(), "a".to_owned()],
    });
    let codes: Vec<&str> = spec.validate().iter().map(|(code, _)| *code).collect();
    assert_eq!(codes, vec!["KLF-DEF-0040", "KLF-DEF-0041"]);
}
