//! Printing a layout: each group flat when it fits in the width left on its
//! line, broken otherwise (Wadler's "prettier printer", with Prettier's
//! look-ahead to the next line break).

use crate::{Doc, FormatError, LineKind, Options};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Flat,
    Break,
}

type Command<'a> = (usize, Mode, &'a Doc);

struct Printer<'a> {
    options: &'a Options,
    out: String,
    column: usize,
    line_start: bool,
    indent: usize,
    blank: bool,
    suffixes: Vec<&'a str>,
    previous: Option<char>,
    gap: bool,
}

/// Prints a layout.
///
/// # Errors
/// Returns [`FormatError::Joined`] when the printed text puts two source
/// tokens together that had whitespace or a comment between them, or splits
/// a word the source had in one piece.
pub fn print(doc: &Doc, options: &Options) -> Result<String, FormatError> {
    let mut printer = Printer {
        options,
        out: String::new(),
        column: 0,
        line_start: true,
        indent: 0,
        blank: false,
        suffixes: Vec::new(),
        previous: None,
        gap: true,
    };
    let mut commands: Vec<Command<'_>> = vec![(0, Mode::Break, doc)];
    while let Some((indent, mode, doc)) = commands.pop() {
        match doc {
            Doc::Token {
                text,
                at,
                separated,
            } => {
                printer.check(text, *at, *separated)?;
                printer.write(text);
            }
            Doc::Text(text) => printer.write(text),
            Doc::Line(kind) => match (mode, kind) {
                (Mode::Flat, LineKind::Soft) => {}
                (Mode::Flat, LineKind::Space) | (_, LineKind::Fixed) => printer.write(" "),
                _ => printer.newline(indent),
            },
            Doc::Group { docs, forced } => {
                let flat = mode == Mode::Flat
                    || !*forced
                        && fits(
                            (indent, Mode::Flat, doc),
                            &commands,
                            printer.options.width.saturating_sub(printer.column),
                        );
                let mode = if flat { Mode::Flat } else { Mode::Break };
                commands.extend(docs.iter().rev().map(|doc| (indent, mode, doc)));
            }
            Doc::Indent { docs, .. } => {
                commands.extend(docs.iter().rev().map(|doc| (indent + 1, mode, doc)));
            }
            Doc::Suffix(text) => printer.suffixes.push(text),
            Doc::Leading(text) => {
                if !printer.line_start {
                    printer.newline(indent);
                }
                printer.indent = indent;
                printer.write(text);
            }
            Doc::Blank => printer.blank = true,
        }
    }
    printer.flush_suffixes();
    let mut out = printer.out;
    let end = out.trim_end().len();
    out.truncate(end);
    let start = out.len() - out.trim_start_matches('\n').len();
    out.drain(..start);
    if options.final_newline && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

impl Printer<'_> {
    fn newline(&mut self, indent: usize) {
        self.flush_suffixes();
        let end = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(end);
        self.out.push('\n');
        self.line_start = true;
        self.indent = indent;
        self.column = 0;
        self.gap = true;
    }

    fn flush_suffixes(&mut self) {
        for suffix in std::mem::take(&mut self.suffixes) {
            self.write(suffix);
        }
    }

    fn write(&mut self, text: &str) {
        if self.line_start {
            if text.trim().is_empty() {
                return;
            }
            if self.blank && !self.out.is_empty() {
                self.out.push('\n');
            }
            let unit = if self.options.insert_spaces {
                " ".repeat(self.options.tab_size)
            } else {
                "\t".to_owned()
            };
            self.out.push_str(&unit.repeat(self.indent));
            self.column = self.indent * self.options.tab_size;
            self.line_start = false;
        }
        self.blank = false;
        self.out.push_str(text);
        match text.rfind('\n') {
            Some(at) => self.column = text[at + 1..].chars().count(),
            None => self.column += text.chars().count(),
        }
        match text.trim_end().chars().last() {
            Some(last) => {
                self.previous = Some(last);
                self.gap = text.ends_with(char::is_whitespace);
            }
            None => self.gap = true,
        }
    }

    fn check(&self, text: &str, at: usize, separated: bool) -> Result<(), FormatError> {
        let (Some(previous), Some(first)) = (self.previous, text.chars().next()) else {
            return Ok(());
        };
        let gap = self.gap || self.line_start;
        let joined = separated
            && !gap
            && (word(previous) && word(first) || symbol(previous) && symbol(first));
        let split = !separated && gap && word(previous) && word(first);
        if joined || split {
            return Err(FormatError::Joined { at });
        }
        Ok(())
    }
}

fn word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn symbol(character: char) -> bool {
    !word(character) && !character.is_whitespace() && !"()[]{},;".contains(character)
}

/// Whether `next`, flat, and whatever follows it up to the next line break
/// fit in `width`.
fn fits(next: Command<'_>, rest: &[Command<'_>], width: usize) -> bool {
    let mut width = isize::try_from(width).unwrap_or(isize::MAX);
    let mut stack = vec![next];
    let mut rest = rest.iter().rev();
    loop {
        let (indent, mode, doc) = match stack.pop() {
            Some(command) => command,
            None => match rest.next() {
                Some(command) => *command,
                None => return true,
            },
        };
        match doc {
            Doc::Token { text, .. } | Doc::Text(text) => {
                if text.contains('\n') {
                    return true;
                }
                width -= isize::try_from(text.chars().count()).unwrap_or(isize::MAX);
            }
            Doc::Line(kind) => match (mode, kind) {
                (_, LineKind::Fixed) | (Mode::Flat, LineKind::Space) => width -= 1,
                (Mode::Break, _) => return true,
                (Mode::Flat, LineKind::Hard) => return false,
                (Mode::Flat, LineKind::Soft) => {}
            },
            Doc::Group { docs, forced } => {
                if *forced && mode == Mode::Flat {
                    return false;
                }
                let mode = if *forced { Mode::Break } else { mode };
                stack.extend(docs.iter().rev().map(|doc| (indent, mode, doc)));
            }
            Doc::Indent { docs, .. } => {
                stack.extend(docs.iter().rev().map(|doc| (indent + 1, mode, doc)));
            }
            Doc::Leading(_) => return mode == Mode::Break,
            Doc::Suffix(_) | Doc::Blank => {}
        }
        if width < 0 {
            return false;
        }
    }
}
