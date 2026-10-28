//! The report a failure prints, laid out as a Rust compiler error (section
//! 31.4): `error[code]: headline`, the place under `-->` with its source line
//! and carets, then `= note:` and `= help:` lines.
//!
//! A trap's report is rendered while the program compiles, from the span its
//! check covers, and printed by the runtime when the check fails. The values
//! the check compared are known only then, so a report may name them as `{0}`
//! and `{1}`, which [`fill_operands`] replaces.

use std::fmt::Write as _;

/// Where a report points: a file position and, when the source is at hand,
/// the line it is on.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Place {
    pub path: String,
    /// One-based.
    pub line: usize,
    /// One-based, in characters.
    pub column: usize,
    /// The whole source line, without its line break.
    pub source_line: Option<String>,
    /// How many characters the carets cover, at least one.
    pub width: usize,
    /// What the carets mark; empty for nothing.
    pub label: String,
}

impl Place {
    /// The place `start..end` covers in `text`, the whole text of the file at
    /// `path`. A range running past its line is cut at the line's end.
    #[must_use]
    pub fn in_text(path: &str, text: &str, start: usize, end: usize, label: &str) -> Self {
        let from = boundary(text, start);
        let line_begin = text[..from].rfind('\n').map_or(0, |newline| newline + 1);
        let line_end = text[line_begin..]
            .find('\n')
            .map_or(text.len(), |newline| line_begin + newline);
        let to = boundary(text, end).max(from).min(line_end);
        Self {
            path: path.to_owned(),
            line: text[..from].matches('\n').count() + 1,
            column: text[line_begin..from].chars().count() + 1,
            source_line: Some(text[line_begin..line_end].trim_end_matches('\r').to_owned()),
            width: text[from..to].chars().count().max(1),
            label: label.to_owned(),
        }
    }
}

/// One failure, ready to print.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Report {
    /// Such as `KLF-RT-0005`.
    pub code: String,
    pub headline: String,
    pub place: Option<Place>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

impl Report {
    /// The report as text, ending in a newline.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = format!("error[{}]: {}\n", self.code, self.headline);
        let digits = self
            .place
            .as_ref()
            .filter(|place| place.source_line.is_some())
            .map_or(1, |place| place.line.to_string().len());
        let pad = " ".repeat(digits);
        if let Some(place) = &self.place {
            let _ = writeln!(out, "{pad}--> {}:{}:{}", place.path, place.line, place.column);
            if let Some(source) = &place.source_line {
                let label = if place.label.is_empty() {
                    String::new()
                } else {
                    format!(" {}", place.label)
                };
                let _ = writeln!(out, "{pad} |");
                let _ = writeln!(out, "{} | {source}", place.line);
                let _ = writeln!(
                    out,
                    "{pad} | {}{}{label}",
                    " ".repeat(place.column.saturating_sub(1)),
                    "^".repeat(place.width.max(1))
                );
            }
        }
        for note in &self.notes {
            part(&mut out, &pad, "note", note);
        }
        for help in &self.helps {
            part(&mut out, &pad, "help", help);
        }
        out
    }
}

fn part(out: &mut String, pad: &str, kind: &str, text: &str) {
    let mut lines = text.split('\n');
    let _ = writeln!(out, "{pad} = {kind}: {}", lines.next().unwrap_or_default());
    for line in lines {
        let _ = writeln!(out, "{pad}   {line}");
    }
}

/// The code a rendered report leads with, such as `KLF-RT-0005`.
#[must_use]
pub fn report_code(rendered: &str) -> Option<&str> {
    let rest = rendered.strip_prefix("error[")?;
    rest.get(..rest.find(']')?)
}

/// `rendered` with `{text}` replaced by `text`, the name a failed check read
/// at run time, such as the type an `Any` held.
#[must_use]
pub fn fill_text(rendered: &str, text: Option<&str>) -> String {
    if !rendered.contains("{text}") {
        return rendered.to_owned();
    }
    rendered.replace("{text}", text.unwrap_or("type the program did not record"))
}

/// A trap as one line of the outcome protocol (`TRAP <line>`): a report's
/// code and headline, or any other message as it is.
#[must_use]
pub fn outcome_line(rendered: &str) -> String {
    match report_code(rendered) {
        Some(code) => {
            let headline = rendered.lines().next().unwrap_or_default();
            let headline = headline.split_once("]: ").map_or(headline, |(_, rest)| rest);
            format!("{code}: {headline}")
        }
        None => rendered.replace('\n', " "),
    }
}

/// `rendered` with `{0}` and `{1}` replaced by the values the failed check
/// compared; a placeholder with no value says so rather than staying.
#[must_use]
pub fn fill_operands(rendered: &str, operands: &[i64]) -> String {
    let mut out = rendered.to_owned();
    for slot in 0..2 {
        let marker = format!("{{{slot}}}");
        if !out.contains(&marker) {
            continue;
        }
        let value = operands
            .get(slot)
            .map_or_else(|| "unknown".to_owned(), ToString::to_string);
        out = out.replace(&marker, &value);
    }
    out
}

/// What a trap's code means, why it happens and how to fix the program
/// (KLF-RIDL section 18.8 for `KLF-RT`; the bytecode VM's own `KLF-VM`
/// codes). Its place is left for the caller, which knows it.
#[must_use]
pub fn trap_description(code: &str) -> Report {
    let (headline, note, help) = match code {
        "KLF-RT-0001" => (
            "reached code the compiler marked unreachable",
            "the compiler ends a path it proved can never run with `unreachable`; running it \
             means that proof was wrong",
            "this is a compiler bug, not a fault in the program: report it with the source of \
             the innermost frame below",
        ),
        "KLF-RT-0002" => (
            "integer overflow: the result does not fit its type",
            "`+`, `-` and `*` on a fixed-width or sized number trap when the exact result leaves \
             the type's range, and so does a shift by more than the width",
            "do the arithmetic in `Number`, check the operands against the range first, or use \
             `wrappingAdd`, `wrappingSub` or `wrappingMul` where wrapping is what you mean",
        ),
        "KLF-RT-0003" => (
            "exclusivity violation: two accesses to the same storage overlap",
            "a value was modified while another access to it was still open",
            "finish the first access before modifying the value, or work on a `copy` of it",
        ),
        "KLF-RT-0004" => (
            "use of a deleted object",
            "the object was deleted while a reference to it was still in use",
            "keep the object alive while references to it remain, or drop them before deleting it",
        ),
        "KLF-RT-0005" => (
            "out-of-bounds access",
            "the access lies outside the memory or the array it reads or writes",
            "check the index or offset against the length before the access",
        ),
        "KLF-RT-0006" => (
            "double free: the slot is already free",
            "raw memory was freed a second time",
            "free each raw allocation exactly once, and forget the pointer once it is freed",
        ),
        "KLF-RT-0010" => (
            "division by zero",
            "integer `/` and `%` trap when the divisor is 0",
            "check the divisor first, or divide `Number`s and handle the zero case",
        ),
        "KLF-RT-0011" => (
            "invalid float-to-integer conversion",
            "the float is NaN, infinite, or outside the integer type's range",
            "check the value with `isFinite` and against the range first, or keep it a `Number`",
        ),
        "KLF-RT-0020" => (
            "an exception reached a RIDL boundary",
            "unwinding never crosses the frame of an exported function (section 16.4)",
            "catch the exception inside the module that raised it, and return a `Result` instead",
        ),
        "KLF-RT-0021" => (
            "stack overflow: the call depth exceeded the thread's stack",
            "a recursion did not stop, or went deeper than the stack allows",
            "check the recursion's base case, or turn the deep recursion into a loop",
        ),
        "KLF-RT-0100" => (
            "heap imbalance at exit: allocations were still live when the program returned",
            "every value the program allocated must be released by the time `main` returns",
            "this is a compiler or runtime leak; rerun with `KLF_HEAP_REPORT=1` for the counts",
        ),
        "KLF-RT-0101" => (
            "reference cycle leak",
            "counted objects that refer to each other were never released",
            "break the cycle with a `weak` reference, or clear one side before dropping the other",
        ),
        "KLF-RT-0200" => (
            "RIDL link check failed: a module imports an ABI its provider does not offer",
            "the provider lists neither the required ABI as current nor as supported (section 24.5)",
            "rebuild the importing module against the provider it runs with",
        ),
        "KLF-VM-0001" => (
            "the VM met an instruction it cannot decode",
            "the bytecode is malformed or was written by another version of the toolchain",
            "rebuild the program with the toolchain that runs it",
        ),
        "KLF-VM-0002" => (
            "the VM ran past the end of a function's code",
            "a function's bytecode ends without a return, which a correct compile never writes",
            "this is a compiler bug: report it with the function's source",
        ),
        "KLF-VM-0003" => (
            "the VM met an instruction it does not run",
            "this VM supports fewer instructions than the compiler that wrote the bytecode",
            "rebuild the program with the toolchain that runs it",
        ),
        "KLF-VM-0004" => (
            "a native symbol the program imports is not there",
            "the VM looked the symbol up in the libraries the program names and found none",
            "add the library that defines it to the package's `nativeLibraries`, or check its name",
        ),
        "KLF-VM-0005" => (
            "a C call's arguments do not match its declaration",
            "the call passes another number or kind of arguments than its C signature takes",
            "check the `@FFI.Extern` declaration against the C prototype",
        ),
        "KLF-VM-0007" => (
            "C called a Kira callback whose entry was already freed",
            "the C side kept a function pointer past the life of the Kira value behind it",
            "keep the callback alive while C may call it, or unregister it before releasing it",
        ),
        "KLF-VM-0022" => (
            "compile-time evaluation ran out of steps",
            "a macro or `comptime` function ran longer than the evaluation budget allows",
            "check the evaluated code for a loop that does not end, or do the work at run time",
        ),
        "KLF-VM-0023" => (
            "compile-time evaluation ran out of memory",
            "a macro or `comptime` function allocated more than the evaluation budget allows",
            "allocate less in the evaluated code, or do the work at run time",
        ),
        "KLF-VM-0024" => (
            "compile-time evaluation called a C function",
            "code a compile evaluates has no host: it cannot call into C",
            "move the C call out of the macro or `comptime` function, to run time",
        ),
        "KLF-VM-0030" => (
            "the run was cancelled",
            "the editor or the driver asked for this run to stop",
            "nothing to fix: start the run again",
        ),
        _ => (
            "the program trapped",
            "the runtime stopped the program with this code",
            "the frames below lead to where it stopped",
        ),
    };
    Report {
        code: code.to_owned(),
        headline: headline.to_owned(),
        place: None,
        notes: vec![note.to_owned()],
        helps: vec![help.to_owned()],
    }
}

/// `offset` as a byte index into `text`: at most its length, on a character
/// boundary at or before it.
fn boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_reads_as_a_compiler_error_with_its_values() {
        let text = "function pick() {\n    return items[index]\n}\n";
        let start = text.find("items").unwrap_or_default();
        let report = Report {
            code: "KLF-RT-0005".to_owned(),
            headline: "index out of range: the index is {0} but the count is {1}".to_owned(),
            place: Some(Place::in_text("app/main.kira", text, start, start + 12, "")),
            notes: vec!["an index must be less than the count".to_owned()],
            helps: vec!["check the index first".to_owned()],
        };
        let rendered = fill_operands(&report.render(), &[5, 2]);
        assert_eq!(
            rendered,
            "error[KLF-RT-0005]: index out of range: the index is 5 but the count is 2\n \
             --> app/main.kira:2:12\n  |\n2 |     return items[index]\n  |            \
             ^^^^^^^^^^^^\n  = note: an index must be less than the count\n  = help: check \
             the index first\n"
        );
        assert_eq!(report_code(&rendered), Some("KLF-RT-0005"));
    }
}
