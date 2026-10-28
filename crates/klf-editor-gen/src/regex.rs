//! Spec patterns in each engine's syntax, and literals as patterns.

use klf_facts::pattern::{Engine, Pattern};

/// `source`, a pattern of the dialect, in `engine`'s syntax.
///
/// # Errors
/// Returns the member name and the pattern error; specs are validated before
/// generation, so this is reached only for an unvalidated spec.
pub(crate) fn translate(member: &str, source: &str, engine: Engine) -> Result<String, String> {
    Pattern::parse(source)
        .map(|pattern| pattern.to_engine(engine))
        .map_err(|error| format!("editor.{member}: {error}"))
}

/// `text` as a pattern matching exactly it, for engines with the usual
/// backslash escapes.
pub(crate) fn literal(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        if character.is_ascii_punctuation() && character != '_' {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// An alternation of literals, longest first so a prefix never wins.
pub(crate) fn alternation(words: &[String]) -> String {
    let mut sorted: Vec<&String> = words.iter().collect();
    sorted.sort_by(|left, right| right.len().cmp(&left.len()).then(left.cmp(right)));
    sorted.dedup();
    sorted
        .iter()
        .map(|word| literal(word))
        .collect::<Vec<_>>()
        .join("|")
}
