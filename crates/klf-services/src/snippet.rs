//! Rust-style rendering of one diagnostic.
//!
//! The headline comes first, then one excerpt per place the diagnostic points
//! at: the primary place under `-->`, the others under `:::`. Each excerpt
//! shows its source line with carets under the range and the label that says
//! what is wrong there. `= note:` and `= help:` lines follow. A message whose
//! later lines already read `= note: ...` or `= help: ...` (the framework's
//! conflict reports do) is split into those parts by `split_message`.

use crate::Session;
use std::fmt::Write as _;
use klf_diag::{Diagnostic, Severity};
use klf_source::Span;

use crate::{ResolvedRuntimeDiagnostic, diagnostic_location, severity_word};

/// One place a diagnostic points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Excerpt {
    /// `name:line:column`, as the arrow line shows it.
    pub location: String,
    /// The whole text of the file the range is in.
    pub text: String,
    pub start: u32,
    pub end: u32,
    /// What is wrong at this place; empty when nothing is said.
    pub label: String,
    pub primary: bool,
}

/// A diagnostic ready to render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendered {
    pub severity: Severity,
    pub code: String,
    pub headline: String,
    pub excerpts: Vec<Excerpt>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

/// Splits a message into its headline, its `= note:` lines and its `= help:`
/// lines. A line after a note or help that does not start with `=` continues
/// it, keeping its indentation.
#[must_use]
pub fn split_message(message: &str) -> (String, Vec<String>, Vec<String>) {
    let mut lines = message.lines();
    let mut headline = lines.next().unwrap_or_default().trim_end().to_owned();
    let mut notes: Vec<String> = Vec::new();
    let mut helps: Vec<String> = Vec::new();
    // Which list the last line went to, so a continuation joins it.
    let mut open: Option<bool> = None;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("= note:") {
            notes.push(rest.trim().to_owned());
            open = Some(false);
        } else if let Some(rest) = trimmed.strip_prefix("= help:") {
            helps.push(rest.trim().to_owned());
            open = Some(true);
        } else {
            match open {
                Some(true) => append_continuation(helps.last_mut(), line),
                Some(false) => append_continuation(notes.last_mut(), line),
                None => {
                    headline.push(' ');
                    headline.push_str(trimmed);
                }
            }
        }
    }
    (headline, notes, helps)
}

fn append_continuation(part: Option<&mut String>, line: &str) {
    if let Some(part) = part {
        part.push('\n');
        part.push_str(line.trim_end());
    }
}

/// The 1-based line and column of byte `offset` in `text`, counting columns in
/// characters. An offset past the end is placed at the end.
#[must_use]
pub fn position(text: &str, offset: u32) -> (usize, usize) {
    let offset = boundary(text, offset);
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

/// `offset` as a byte index into `text`: at most the text's length, and on a
/// character boundary at or before the requested place.
fn boundary(text: &str, offset: u32) -> usize {
    let mut offset = usize::try_from(offset).unwrap_or(usize::MAX).min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The line `text` holds at one-based `line`, without its line break.
fn line_text(text: &str, line: usize) -> &str {
    text.split('\n')
        .nth(line.saturating_sub(1))
        .unwrap_or_default()
        .trim_end_matches('\r')
}

/// The one-based line, the column and the number of carets that cover the
/// range `start..end` on its line. The carets stop at the line's end.
fn caret_span(text: &str, start: u32, end: u32) -> (usize, usize, usize) {
    let (line, column) = position(text, start);
    let from = boundary(text, start);
    let line_begin = text[..from].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[line_begin..]
        .find('\n')
        .map_or(text.len(), |newline| line_begin + newline);
    let to = boundary(text, end).max(from).min(line_end);
    let width = text.get(from..to).map_or(0, |range| range.chars().count()).max(1);
    (line, column, width)
}

fn digits(number: usize) -> usize {
    number.to_string().len()
}

/// The rendered diagnostic, one trailing newline included.
#[must_use]
pub fn render(diagnostic: &Rendered) -> String {
    let shown: Vec<(usize, usize, usize)> = diagnostic
        .excerpts
        .iter()
        .map(|excerpt| caret_span(&excerpt.text, excerpt.start, excerpt.end))
        .collect();
    let width = shown.iter().map(|(line, _, _)| digits(*line)).max().unwrap_or(1);
    let pad = " ".repeat(width);
    // The primary place leads the headline as `path:line:column: error[code]:`,
    // the form the corpus runner and the editors read; a secondary place gets
    // its own `:::` line.
    let mut out = match diagnostic.excerpts.iter().find(|excerpt| excerpt.primary) {
        Some(primary) => format!(
            "{}: {}[{}]: {}\n",
            primary.location,
            severity_word(diagnostic.severity),
            diagnostic.code,
            diagnostic.headline
        ),
        None => format!(
            "{}[{}]: {}\n",
            severity_word(diagnostic.severity),
            diagnostic.code,
            diagnostic.headline
        ),
    };
    for (excerpt, (line, column, carets)) in diagnostic.excerpts.iter().zip(&shown) {
        if !excerpt.primary {
            let _ = writeln!(out, "{pad} ::: {}", excerpt.location);
        }
        let _ = writeln!(out, "{pad} |");
        let source = line_text(&excerpt.text, *line);
        let _ = writeln!(out, "{line:>width$} | {source}");
        let label = if excerpt.label.is_empty() {
            String::new()
        } else {
            format!(" {}", excerpt.label)
        };
        let _ = writeln!(
            out,
            "{pad} | {}{}{label}",
            " ".repeat(column - 1),
            "^".repeat(*carets)
        );
    }
    for note in &diagnostic.notes {
        push_part(&mut out, &pad, "note", note);
    }
    for help in &diagnostic.helps {
        push_part(&mut out, &pad, "help", help);
    }
    out
}

fn push_part(out: &mut String, pad: &str, kind: &str, text: &str) {
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or_default();
    let _ = writeln!(out, "{pad} = {kind}: {first}");
    for line in lines {
        let _ = writeln!(out, "{pad}   {line}");
    }
}

/// Whether `text` may be quoted in a headline: a name or one offending token,
/// written without spaces and short enough to read in a headline. Longer or
/// multi-token text, such as a whole expression, is left to the title.
fn quotable(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && !text.chars().any(char::is_whitespace)
}

/// A runtime diagnostic a definition reported, rendered from its span. The
/// headline names the source identifier under the span when it is one, so
/// `undefined name` reads as ``undefined name `total` ``.
#[must_use]
pub fn runtime_diagnostic(session: &Session, diagnostic: &ResolvedRuntimeDiagnostic) -> String {
    let span = diagnostic.span;
    let location = diagnostic_location(session, span.file, span.start);
    if !session.has_file(span.file) {
        return format!(
            "{location}: {}[{}]: {}: {}\n",
            severity_word(diagnostic.severity),
            diagnostic.code,
            diagnostic.title,
            diagnostic.explanation
        );
    }
    let text = session.file(span.file).text().to_owned();
    let named = diagnostic.subject.unwrap_or(span);
    let quoted = text
        .get(usize::try_from(named.start).unwrap_or(0)..usize::try_from(named.end).unwrap_or(0))
        .filter(|name| quotable(name) && !diagnostic.title.contains('`'));
    let headline = quoted.map_or_else(
        || diagnostic.title.clone(),
        |name| format!("{} `{name}`", diagnostic.title),
    );
    let mut excerpts = vec![Excerpt {
        location,
        text,
        start: span.start,
        end: span.end,
        label: diagnostic.explanation.clone(),
        primary: true,
    }];
    let mut notes = diagnostic.notes.clone();
    for (place, label) in &diagnostic.places {
        if session.has_file(place.file) {
            excerpts.push(Excerpt {
                location: diagnostic_location(session, place.file, place.start),
                text: session.file(place.file).text().to_owned(),
                start: place.start,
                end: place.end,
                label: label.clone(),
                primary: false,
            });
        } else {
            notes.push(label.clone());
        }
    }
    notes.extend(generated_origin_notes(session, span.file));
    let helps = if diagnostic.helps.is_empty() {
        (!diagnostic.help.is_empty())
            .then(|| diagnostic.help.clone())
            .into_iter()
            .collect()
    } else {
        diagnostic.helps.clone()
    };
    render(&Rendered {
        severity: diagnostic.severity,
        code: diagnostic.code.to_string(),
        headline,
        excerpts,
        notes,
        helps,
    })
}

/// The virtual file marker `diagnostic_location` maps through (`name#generated`).
const GENERATED_MARKER: &str = "#generated";

/// A note tracing a place in generated text back to the file it was generated
/// from, when that file is registered. Empty for a file written by hand.
#[must_use]
pub fn generated_origin_notes(session: &Session, file: klf_core::FileId) -> Vec<String> {
    if !session.has_file(file) {
        return Vec::new();
    }
    let name = session.file(file).name().as_str().to_owned();
    let Some((origin, _)) = name.rsplit_once(GENERATED_MARKER) else {
        return Vec::new();
    };
    if !session.files().iter().any(|source| source.name().as_str() == origin) {
        return Vec::new();
    }
    vec![format!(
        "this code is generated from `{origin}`; the place above is its generated text"
    )]
}

/// A diagnostic a phase built with labels and notes, rendered from its spans.
/// The first label is the primary place; a label or note with a span of its
/// own is shown as a further place.
#[must_use]
pub fn session_diagnostic(session: &Session, diagnostic: &Diagnostic) -> String {
    let (headline, notes, helps) = split_message(&diagnostic.message);
    let mut excerpts = Vec::new();
    let primary = diagnostic.primary_span();
    let mut primary_done = false;
    for label in &diagnostic.labels {
        let is_primary = !primary_done && Some(label.span) == primary;
        primary_done |= is_primary;
        push_excerpt(session, &mut excerpts, label.span, &label.message, is_primary);
    }
    if !primary_done {
        if let Some(span) = primary {
            push_excerpt(session, &mut excerpts, span, "", true);
        }
    }
    for note in &diagnostic.notes {
        if let Some(span) = note.span {
            push_excerpt(session, &mut excerpts, span, &note.message, false);
        }
    }
    let notes: Vec<String> = notes
        .into_iter()
        .chain(
            diagnostic
                .notes
                .iter()
                .filter(|note| note.span.is_none())
                .map(|note| note.message.clone()),
        )
        .collect();
    let mut notes = notes;
    if let Some(span) = primary {
        notes.extend(generated_origin_notes(session, span.file));
    }
    render(&Rendered {
        severity: diagnostic.severity,
        code: diagnostic.code.to_owned(),
        headline,
        excerpts,
        notes,
        helps,
    })
}

fn push_excerpt(
    session: &Session,
    excerpts: &mut Vec<Excerpt>,
    span: Span,
    label: &str,
    primary: bool,
) {
    if !session.has_file(span.file) {
        return;
    }
    excerpts.push(Excerpt {
        location: diagnostic_location(session, span.file, span.start),
        text: session.file(span.file).text().to_owned(),
        start: span.start,
        end: span.end,
        label: label.to_owned(),
        primary,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_diag::Severity;

    fn excerpt(text: &str, start: u32, end: u32, label: &str, primary: bool) -> Excerpt {
        Excerpt {
            location: "app/main.kira:5:21".to_owned(),
            text: text.to_owned(),
            start,
            end,
            label: label.to_owned(),
            primary,
        }
    }

    const SOURCE: &str = "import Foundation\n\n@Main\nfunction main() {\n    let x: String = missingThing(3)\n    print(x)\n}\n";

    #[test]
    fn undefined_function_is_rendered_with_its_carets() {
        let start = u32::try_from(SOURCE.find("missingThing").unwrap()).unwrap();
        let rendered = render(&Rendered {
            severity: Severity::Error,
            code: "3061".to_owned(),
            headline: "call to an undefined function `missingThing`".to_owned(),
            excerpts: vec![excerpt(SOURCE, start, start + 12, "no function with this name is declared", true)],
            notes: Vec::new(),
            helps: Vec::new(),
        });
        let expected = format!(
            "app/main.kira:5:21: error[3061]: call to an undefined function `missingThing`\n\
             {pad}|\n\
             5 |     let x: String = missingThing(3)\n\
             {pad}|{spaces}{carets} no function with this name is declared\n",
            pad = "  ",
            spaces = " ".repeat(21),
            carets = "^".repeat(12),
        );
        assert_eq!(rendered, expected);
    }

    #[test]
    fn secondary_place_notes_and_helps_follow_the_excerpts() {
        let rendered = render(&Rendered {
            severity: Severity::Error,
            code: "KLF-KIR-9002".to_owned(),
            headline: "defines `count` in more than one unit".to_owned(),
            excerpts: vec![
                excerpt(SOURCE, 0, 6, "first definition", true),
                excerpt(SOURCE, 7, 17, "second definition", false),
            ],
            notes: vec!["they differ in their linkage".to_owned()],
            helps: vec!["a program defines each name once".to_owned()],
        });
        assert!(rendered.starts_with(
            "app/main.kira:5:21: error[KLF-KIR-9002]: defines `count` in more than one unit\n"
        ));
        assert!(rendered.contains("\n  ::: app/main.kira:5:21\n"));
        assert!(rendered.ends_with(
            "  = note: they differ in their linkage\n  = help: a program defines each name once\n"
        ));
    }

    #[test]
    fn wide_line_numbers_widen_the_gutter() {
        let text = format!("{}x", "\n".repeat(9));
        let rendered = render(&Rendered {
            severity: Severity::Error,
            code: "2001".to_owned(),
            headline: "unexpected token".to_owned(),
            excerpts: vec![excerpt(&text, 9, 10, "", true)],
            notes: Vec::new(),
            helps: Vec::new(),
        });
        assert!(rendered.contains("\n10 | x\n"));
        assert!(rendered.contains("\n   | ^\n"));
    }

    #[test]
    fn a_message_splits_into_headline_notes_and_helps() {
        let message = "defines `f` in more than one unit\n  = note: unit `a` defines it\n  \
                       = note: they differ in\n    against\n  = help: name it once";
        let (headline, notes, helps) = split_message(message);
        assert_eq!(headline, "defines `f` in more than one unit");
        assert_eq!(notes, vec!["unit `a` defines it".to_owned(), "they differ in\n    against".to_owned()]);
        assert_eq!(helps, vec!["name it once".to_owned()]);
    }

    #[test]
    fn carets_stop_at_the_end_of_their_line() {
        let text = "let a = 1\nlet b = 2\n";
        assert_eq!(caret_span(text, 4, 40), (1, 5, 5));
        assert_eq!(caret_span(text, 10, 10), (2, 1, 1));
    }

    fn session_with_source() -> (Session, klf_core::FileId) {
        let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
        let file = session.add_file("app/main.kira", SOURCE);
        (session, file)
    }

    #[test]
    fn a_runtime_record_renders_through_its_spec_and_file() {
        let (mut session, file) = session_with_source();
        assert!(session.register_diagnostic(crate::DiagnosticSpec::new(
            3061,
            Severity::Error,
            "call to an undefined function",
            "no function with this name is declared",
        )));
        let start = i64::try_from(SOURCE.find("missingThing").unwrap()).unwrap();
        let record = klf_diag::RuntimeDiagnostic::new(3061, start, start + 12);
        session
            .report_runtime_diagnostics(file, &[record])
            .expect("the spec is registered");
        assert_eq!(
            runtime_diagnostic(&session, &session.runtime_diagnostics()[0]),
            "app/main.kira:5:21: error[3061]: call to an undefined function `missingThing`\n\
             \u{20}\u{20}|\n\
             5 |     let x: String = missingThing(3)\n\
             \u{20}\u{20}|                     ^^^^^^^^^^^^ no function with this name is declared\n"
        );
    }

    #[test]
    fn a_labelled_diagnostic_shows_each_place_and_its_notes() {
        let (session, file) = session_with_source();
        let first = u32::try_from(SOURCE.find("missingThing").unwrap()).unwrap();
        let second = u32::try_from(SOURCE.find("let x").unwrap()).unwrap();
        let diagnostic = klf_diag::Diagnostic::error_at(
            "KLF-KIR-9002",
            Span::new(file, first, first + 12),
            "defines `count` in more than one unit\n  = note: unit `a` defines it\n  \
             = help: name it once",
        )
        .with_label(Span::new(file, first, first + 12), "first definition")
        .with_label(Span::new(file, second, second + 3), "second definition");
        assert_eq!(
            session_diagnostic(&session, &diagnostic),
            "app/main.kira:5:21: error[KLF-KIR-9002]: defines `count` in more than one unit\n\
             \u{20}\u{20}|\n\
             5 |     let x: String = missingThing(3)\n\
             \u{20}\u{20}|                     ^^^^^^^^^^^^ first definition\n\
             \u{20}\u{20}::: app/main.kira:5:5\n\
             \u{20}\u{20}|\n\
             5 |     let x: String = missingThing(3)\n\
             \u{20}\u{20}|     ^^^ second definition\n\
             \u{20}\u{20}= note: unit `a` defines it\n\
             \u{20}\u{20}= help: name it once\n"
        );
    }

    #[test]
    fn generated_text_names_the_file_it_came_from() {
        let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
        let origin = session.add_file("app/main.kira", SOURCE);
        let generated = session.add_file("app/main.kira#generated1", SOURCE);
        assert!(generated_origin_notes(&session, origin).is_empty());
        assert_eq!(
            generated_origin_notes(&session, generated),
            vec![
                "this code is generated from `app/main.kira`; the place above is its generated text"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn subjects_quote_one_token_only() {
        assert!(quotable("\""));
        assert!(quotable("/*"));
        assert!(quotable("missingThing"));
        assert!(!quotable(""));
        assert!(!quotable("x + 1"));
        assert!(!quotable("a\nb"));
        assert!(!quotable(&"x".repeat(65)));
    }
}
