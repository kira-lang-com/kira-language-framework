//! The markers of an editor corpus text (`KLF-EDITOR.md` Appendix A): `$0`
//! the cursor, `$<name>` a named position, `$[name` … `$]` a named range.
//! They are removed before the file is opened, and each is kept as the byte
//! offsets it marks in the text left.

use std::collections::HashMap;

/// What a marker marks: a file of the case and byte offsets in its text.
/// A position has `start == end`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Mark {
    pub(crate) file: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// The text of file `file` with its markers removed, adding each marker to
/// `marks` under its name, `$0` for the cursor.
pub(crate) fn strip(
    text: &str,
    file: usize,
    marks: &mut HashMap<String, Mark>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find('$') {
        out.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        let mut add = |name: String, start: usize, end: usize| {
            if marks
                .insert(name.clone(), Mark { file, start, end })
                .is_some()
            {
                return Err(format!("the marker `{name}` is used twice"));
            }
            Ok(())
        };
        if let Some(tail) = after.strip_prefix('0') {
            add("$0".to_owned(), out.len(), out.len())?;
            rest = tail;
        } else if let Some(tail) = after.strip_prefix('<') {
            let close = tail.find('>').ok_or("a `$<` marker has no `>`")?;
            add(tail[..close].to_owned(), out.len(), out.len())?;
            rest = &tail[close + 1..];
        } else if let Some(tail) = after.strip_prefix('[') {
            let length = tail
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .unwrap_or(tail.len());
            if length == 0 {
                return Err("a `$[` marker has no name".into());
            }
            let name = &tail[..length];
            if open.iter().any(|(open, _)| open == name) {
                return Err(format!("the range `{name}` is nested in itself"));
            }
            open.push((name.to_owned(), out.len()));
            rest = &tail[length..];
        } else if let Some(tail) = after.strip_prefix(']') {
            let (name, start) = open.pop().ok_or("a `$]` closes no range")?;
            add(name, start, out.len())?;
            rest = tail;
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    match open.pop() {
        Some((name, _)) => Err(format!("the range `{name}` is not closed")),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::{Mark, strip};
    use std::collections::HashMap;

    #[test]
    fn markers_are_removed_and_kept_as_offsets() {
        let mut marks = HashMap::new();
        let text = strip("let $<x>value = $[all$0val$] + $1", 2, &mut marks).expect("strips");
        assert_eq!(text, "let value = val + $1");
        assert_eq!(
            marks["x"],
            Mark {
                file: 2,
                start: 4,
                end: 4
            }
        );
        assert_eq!(
            marks["$0"],
            Mark {
                file: 2,
                start: 12,
                end: 12
            }
        );
        assert_eq!(
            marks["all"],
            Mark {
                file: 2,
                start: 12,
                end: 15
            }
        );
        assert!(strip("$[a $[a $] $]", 0, &mut HashMap::new()).is_err());
        assert!(strip("$[a", 0, &mut HashMap::new()).is_err());
    }
}
