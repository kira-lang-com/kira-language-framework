//! Where a diagnostic is shown.
//!
//! A diagnostic inside a virtual file (section 8.1) points into generated
//! text. Where that text is a line copied unchanged from the file the
//! expansion came from, the diagnostic is shown at that line of the origin
//! (KLF-EDITOR.md, section 4, item 4); a line the expansion wrote itself has
//! no origin of its own and keeps its place in the virtual file.

use klf_core::FileId;
use klf_source::SourceFile;

use crate::Session;

/// The suffix a virtual file's name carries after its origin's name.
const GENERATED_MARKER: &str = "#generated";

/// `name:line:column` for `offset` in `file`, at the origin when the line
/// holding it was copied there unchanged.
#[must_use]
pub fn diagnostic_location(session: &Session, file: FileId, offset: u32) -> String {
    let source = session.file(file);
    if let Some(origin) = origin_file(session, source)
        && let Some(location) = origin_location(source, origin, offset)
    {
        return location;
    }
    let position = source.line_column(offset);
    format!("{}:{}:{}", source.name(), position.line, position.column)
}

/// The file a virtual file was generated from, named by the virtual file's
/// name up to its marker.
fn origin_file<'a>(session: &'a Session, source: &SourceFile) -> Option<&'a SourceFile> {
    let name = source.name().as_str();
    let (origin, _) = name.rsplit_once(GENERATED_MARKER)?;
    session
        .files()
        .iter()
        .find(|file| file.name().as_str() == origin)
}

fn origin_location(source: &SourceFile, origin: &SourceFile, offset: u32) -> Option<String> {
    let position = source.line_column(offset);
    let generated: Vec<&str> = source.text().split('\n').collect();
    let written: Vec<&str> = origin.text().split('\n').collect();
    let line = usize::try_from(position.line).ok()?.checked_sub(1)?;
    let matched = matching_line(&generated, &written, line)?;
    Some(format!(
        "{}:{}:{}",
        origin.name(),
        matched + 1,
        position.column
    ))
}

/// The line of `written` that `generated[line]` was copied from.
///
/// Lines that occur exactly once in each text anchor the two to each other;
/// the copy of a line between two anchors lies between their origins, at the
/// same distance from the anchor before it when that line is the same text.
fn matching_line(generated: &[&str], written: &[&str], line: usize) -> Option<usize> {
    let text = *generated.get(line)?;
    if text.trim().is_empty() {
        return None;
    }
    let anchors = anchors(generated, written);
    if let Some(&(_, origin)) = anchors.iter().find(|(at, _)| *at == line) {
        return Some(origin);
    }
    let before = anchors.iter().rev().find(|(at, _)| *at < line);
    let after = anchors.iter().find(|(at, _)| *at > line);
    let low = before.map_or(0, |(_, origin)| origin + 1);
    let high = after.map_or(written.len(), |(_, origin)| *origin);
    if let Some((at, origin)) = before {
        let guess = origin + (line - at);
        if guess < high && written[guess] == text {
            return Some(guess);
        }
    }
    (low..high).find(|&candidate| written[candidate] == text)
}

/// `(generated line, origin line)` for each non-blank line that occurs once in
/// each text, kept only while both sides rise, so the anchors read in order.
fn anchors(generated: &[&str], written: &[&str]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for (at, text) in generated.iter().enumerate() {
        if text.trim().is_empty() || occurrences(generated, text) != 1 {
            continue;
        }
        let mut found = written
            .iter()
            .enumerate()
            .filter(|(_, candidate)| *candidate == text);
        if let (Some((origin, _)), None) = (found.next(), found.next()) {
            pairs.push((at, origin));
        }
    }
    let mut ordered: Vec<(usize, usize)> = Vec::new();
    for pair in pairs {
        if ordered.last().is_none_or(|last| pair.1 > last.1) {
            ordered.push(pair);
        }
    }
    ordered
}

fn occurrences(lines: &[&str], text: &str) -> usize {
    lines.iter().filter(|line| **line == text).count()
}

#[cfg(test)]
mod tests {
    use super::matching_line;

    #[test]
    fn copied_line_maps_to_its_origin() {
        let written = [
            "struct A {",
            "    var x: Int",
            "    body {",
            "        Grid()",
            "    }",
            "}",
        ];
        let generated = [
            "struct __pw_A_x {",
            "    var wrappedValue: Int",
            "}",
            "struct A {",
            "    var x: Int {",
            "        get { return 0 }",
            "    }",
            "    body {",
            "        Grid()",
            "    }",
            "}",
        ];
        assert_eq!(matching_line(&generated, &written, 8), Some(3));
    }

    #[test]
    fn written_line_has_no_origin() {
        let written = ["struct A {", "    body {", "    }", "}"];
        let generated = [
            "struct A {",
            "        get { return 0 }",
            "    body {",
            "    }",
            "}",
        ];
        assert_eq!(matching_line(&generated, &written, 1), None);
    }

    #[test]
    fn repeated_line_follows_its_anchor() {
        let written = ["a {", "    return", "}", "b {", "    return", "}"];
        let generated = ["x", "a {", "    return", "}", "y", "b {", "    return", "}"];
        assert_eq!(matching_line(&generated, &written, 6), Some(4));
    }
}
