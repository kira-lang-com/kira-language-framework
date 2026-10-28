//! The edits that turn a source into its formatted text.

/// Replace the source's `start..end` (byte offsets) with `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// The edits from `source` to `formatted`: one per whitespace run that
/// changed, so an editor keeps its cursor and marks. When comments moved, so
/// the two texts do not line up by their non-whitespace characters, one edit
/// covering the changed middle.
#[must_use]
pub fn edits(source: &str, formatted: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let (mut at, mut to) = (0, 0);
    loop {
        let old = whitespace(source, at);
        let new = whitespace(formatted, to);
        if source[at..old] != formatted[to..new] {
            edits.push(TextEdit {
                start: at,
                end: old,
                text: formatted[to..new].to_owned(),
            });
        }
        (at, to) = (old, new);
        match (source[at..].chars().next(), formatted[to..].chars().next()) {
            (None, None) => return edits,
            (Some(left), Some(right)) if left == right => {
                at += left.len_utf8();
                to += right.len_utf8();
            }
            _ => return whole(source, formatted),
        }
    }
}

fn whitespace(text: &str, at: usize) -> usize {
    text[at..]
        .find(|character: char| !character.is_whitespace())
        .map_or(text.len(), |end| at + end)
}

fn whole(source: &str, formatted: &str) -> Vec<TextEdit> {
    let prefix = source
        .char_indices()
        .zip(formatted.chars())
        .find(|((_, left), right)| left != right)
        .map_or(source.len().min(formatted.len()), |((at, _), _)| at);
    let suffix = source[prefix..]
        .chars()
        .rev()
        .zip(formatted[prefix..].chars().rev())
        .take_while(|(left, right)| left == right)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    if prefix == source.len() && prefix == formatted.len() {
        return Vec::new();
    }
    vec![TextEdit {
        start: prefix,
        end: source.len() - suffix,
        text: formatted[prefix..formatted.len() - suffix].to_owned(),
    }]
}

/// The edits touching the source's `start..end`, for range formatting.
#[must_use]
pub fn restrict(edits: Vec<TextEdit>, start: usize, end: usize) -> Vec<TextEdit> {
    edits
        .into_iter()
        .filter(|edit| edit.start <= end && edit.end >= start)
        .collect()
}
