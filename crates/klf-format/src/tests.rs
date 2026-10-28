use crate::{Builder, Doc, FormatError, LineKind, Options, Syntax, edits, print};

fn syntax() -> Syntax {
    Syntax {
        line_comments: vec!["//".to_owned()],
        block_comments: vec![("/*".to_owned(), "*/".to_owned(), false)],
    }
}

/// `name(arg, ...);` statements, one per line, each call a group.
fn calls(source: &str) -> Result<Doc, FormatError> {
    let mut builder = Builder::new(source, syntax());
    let masked = mask(source);
    let words: Vec<(usize, &str)> = masked
        .match_indices(|c: char| c.is_alphanumeric() || "(),;".contains(c))
        .collect();
    let mut index = 0;
    let mut first = true;
    while index < words.len() {
        if !first {
            builder.line(LineKind::Hard);
        }
        first = false;
        let mut name = String::new();
        while index < words.len() && words[index].1 != "(" {
            name.push_str(words[index].1);
            index += 1;
        }
        builder.text(&name);
        builder.text("(");
        builder.begin_group();
        builder.begin_indent();
        builder.line(LineKind::Soft);
        index += 1;
        while index < words.len() && words[index].1 != ")" {
            let mut argument = String::new();
            while index < words.len() && !",)".contains(words[index].1) {
                argument.push_str(words[index].1);
                index += 1;
            }
            builder.text(&argument);
            if words.get(index).is_some_and(|word| word.1 == ",") {
                builder.text(",");
                builder.line(LineKind::Space);
                index += 1;
            }
        }
        builder.end();
        builder.line(LineKind::Soft);
        builder.end();
        builder.text(")");
        builder.text(";");
        index += 2;
    }
    builder.finish()
}

/// The source with its comments blanked out, so the test's tokenizer skips
/// them as the definition's lexer would.
fn mask(source: &str) -> String {
    let mut masked = String::new();
    let mut rest = source;
    while !rest.is_empty() {
        let end = if rest.starts_with("//") {
            rest.find('\n').unwrap_or(rest.len())
        } else if rest.starts_with("/*") {
            rest.find("*/").map_or(rest.len(), |end| end + 2)
        } else {
            masked.push(rest.chars().next().unwrap_or(' '));
            rest = &rest[rest.chars().next().map_or(1, char::len_utf8)..];
            continue;
        };
        masked.push_str(&" ".repeat(end));
        rest = &rest[end..];
    }
    masked
}

fn format(source: &str, width: usize) -> String {
    let options = Options {
        width,
        ..Options::default()
    };
    print(&calls(source).expect("layout"), &options).expect("print")
}

#[test]
fn groups_break_only_when_too_wide() {
    let source = "f(alpha,beta);\ng(  gamma ,delta )  ;";
    assert_eq!(format(source, 80), "f(alpha, beta);\ng(gamma, delta);\n");
    assert_eq!(
        format(source, 10),
        "f(\n    alpha,\n    beta\n);\ng(\n    gamma,\n    delta\n);\n"
    );
}

#[test]
fn comments_and_blank_lines_survive() {
    let source = "// head\n\nf(a, // first\n  b);\n\n\n/* gap */ g(c) /* tail */;\n// end\n";
    let formatted = format(source, 80);
    assert_eq!(
        formatted,
        "// head\n\nf(\n    a, // first\n    b\n);\n\n/* gap */ g(c) /* tail */;\n// end\n"
    );
    assert_eq!(format(&formatted, 80), formatted);
}

#[test]
fn layouts_that_change_tokens_are_rejected() {
    let mut builder = Builder::new("f(a);", syntax());
    builder.text("f");
    assert!(!builder.text("["));
    assert!(matches!(
        builder.finish(),
        Err(FormatError::Mismatch { at: 1, .. })
    ));

    let mut builder = Builder::new("f(a); g", syntax());
    for token in ["f", "(", "a", ")", ";"] {
        builder.text(token);
    }
    assert_eq!(builder.finish(), Err(FormatError::Unconsumed { at: 6 }));

    let mut builder = Builder::new("a b", syntax());
    builder.text("a");
    builder.text("b");
    let doc = builder.finish().expect("layout");
    assert_eq!(
        print(&doc, &Options::default()),
        Err(FormatError::Joined { at: 2 })
    );
}

#[test]
fn edits_touch_only_whitespace() {
    let source = "f(  a,b ) ;\n";
    let formatted = format(source, 80);
    let edits = edits(source, &formatted);
    assert_eq!(edits.len(), 4);
    let mut applied = source.to_owned();
    for edit in edits.iter().rev() {
        applied.replace_range(edit.start..edit.end, &edit.text);
    }
    assert_eq!(applied, formatted);
}

#[test]
fn a_byte_order_mark_is_skipped_and_kept() {
    let source = "\u{feff}f( a );\n";
    let doc = calls(source).expect("layout");
    let text = crate::print_source(source, &doc, &Options::default()).expect("print");
    assert_eq!(text, "\u{feff}f(a);\n");
}
