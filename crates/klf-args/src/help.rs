//! Help and usage text, laid out as Swift `ArgumentParser` lays it out.

use crate::parse::command_names;
use crate::{Action, Arg, ArgKind, Arity, Command, ValueType};
use std::fmt::Write as _;
use std::io::IsTerminal;

const WIDTH: usize = 80;
const COLUMN: usize = 26;

/// The usage line of the command at `path`.
#[must_use]
pub fn usage(root: &Command, path: &[usize]) -> String {
    format!("Usage: {}", usage_body(root, path))
}

fn usage_body(root: &Command, path: &[usize]) -> String {
    let command = root.at(path);
    let names = command_names(root, path).join(" ");
    let mut parts: Vec<String> = command
        .args
        .iter()
        .filter(|arg| !arg.hidden)
        .filter(|arg| matches!(arg.kind, ArgKind::Flag | ArgKind::Option))
        .map(usage_part)
        .collect();
    let line = format!("{names} {}", parts.join(" "));
    if line.len() > WIDTH - "Usage: ".len() {
        parts = vec!["[<options>]".to_owned()];
    }
    parts.extend(
        command
            .args
            .iter()
            .filter(|arg| !arg.hidden && matches!(arg.kind, ArgKind::Positional | ArgKind::Rest))
            .map(usage_part),
    );
    if !command.subcommands.is_empty() {
        parts.push(
            if command.action == Action::None && command.default_subcommand.is_none() {
                "<subcommand>".to_owned()
            } else {
                "[<subcommand>]".to_owned()
            },
        );
    }
    if parts.is_empty() {
        names
    } else {
        format!("{names} {}", parts.join(" "))
    }
}

fn usage_part(arg: &Arg) -> String {
    let spelled = match arg.kind {
        ArgKind::Flag => spelling(arg),
        ArgKind::Option => format!("{} <{}>", spelling(arg), arg.placeholder()),
        ArgKind::Positional => format!("<{}>", arg.placeholder()),
        ArgKind::Rest => format!("-- <{}>", arg.placeholder()),
    };
    let many = if arg.arity == Arity::Many { " ..." } else { "" };
    if arg.required {
        format!("{spelled}{many}")
    } else {
        format!("[{spelled}{many}]")
    }
}

fn spelling(arg: &Arg) -> String {
    match (&arg.long, arg.short) {
        (Some(long), _) => format!("--{long}"),
        (None, Some(short)) => format!("-{short}"),
        (None, None) => format!("--{}", crate::kebab_case(&arg.id)),
    }
}

/// The full help of the command at `path`, styled when standard output is a
/// terminal and `NO_COLOR` is unset.
#[must_use]
pub fn help(root: &Command, path: &[usize]) -> String {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    render(root, path, color)
}

pub(crate) fn render(root: &Command, path: &[usize], color: bool) -> String {
    let command = root.at(path);
    let heading = |text: &str| {
        if color {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    };
    let mut out = String::new();
    if !command.abstract_text.is_empty() {
        let _ = write!(
            out,
            "{} {}\n\n",
            heading("OVERVIEW:"),
            command.abstract_text
        );
    }
    if !command.discussion.is_empty() {
        out.push_str(&command.discussion);
        out.push_str("\n\n");
    }
    let _ = writeln!(out, "{} {}", heading("USAGE:"), usage_body(root, path));

    let visible = |kinds: &[ArgKind]| -> Vec<&Arg> {
        command
            .args
            .iter()
            .filter(|arg| !arg.hidden && kinds.contains(&arg.kind))
            .collect()
    };
    let arguments = visible(&[ArgKind::Positional, ArgKind::Rest]);
    if !arguments.is_empty() {
        let _ = write!(out, "\n{}\n", heading("ARGUMENTS:"));
        for arg in arguments {
            row(
                &mut out,
                &format!("<{}>", arg.placeholder()),
                &describe(arg),
            );
        }
    }

    let _ = write!(out, "\n{}\n", heading("OPTIONS:"));
    for arg in visible(&[ArgKind::Flag, ArgKind::Option]) {
        row(&mut out, &option_label(arg), &describe(arg));
    }
    if path.is_empty() && root.version.is_some() {
        row(&mut out, "--version", "Show the version.");
    }
    row(&mut out, "-h, --help", "Show help information.");

    let subcommands: Vec<&Command> = command.subcommands.iter().filter(|c| !c.hidden).collect();
    if !subcommands.is_empty() {
        let _ = write!(out, "\n{}\n", heading("SUBCOMMANDS:"));
        for subcommand in &subcommands {
            let mut label = subcommand.name.clone();
            if command.default_subcommand.as_deref() == Some(subcommand.name.as_str()) {
                label.push_str(" (default)");
            }
            row(&mut out, &label, &subcommand.abstract_text);
        }
        let _ = write!(
            out,
            "\n  See '{} help <subcommand>' for detailed help.\n",
            command_names(root, path).join(" ")
        );
    }
    out
}

fn option_label(arg: &Arg) -> String {
    let mut names = Vec::new();
    if let Some(short) = arg.short {
        names.push(format!("-{short}"));
    }
    if let Some(long) = &arg.long {
        if arg.kind == ArgKind::Flag && arg.value == ValueType::Bool {
            names.push(format!("--[no-]{long}"));
        } else {
            names.push(format!("--{long}"));
        }
    }
    names.extend(arg.aliases.iter().map(|alias| format!("--{alias}")));
    let mut label = names.join(", ");
    if arg.kind == ArgKind::Option {
        let _ = write!(label, " <{}>", arg.placeholder());
    }
    label
}

fn describe(arg: &Arg) -> String {
    let mut text = arg.help.clone();
    let mut notes = Vec::new();
    if let ValueType::Choice(names) = &arg.value {
        notes.push(format!("values: {}", names.join(", ")));
    }
    if let Some(default) = &arg.default {
        notes.push(format!("default: {default}"));
    }
    if !notes.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        let _ = write!(text, "({})", notes.join("; "));
    }
    text
}

/// One `label  text` row, the text wrapped at the help column.
fn row(out: &mut String, label: &str, text: &str) {
    let indent = " ".repeat(COLUMN);
    let mut line = format!("  {label}");
    if text.is_empty() {
        out.push_str(&line);
        out.push('\n');
        return;
    }
    if line.len() + 2 > COLUMN {
        out.push_str(&line);
        out.push('\n');
        line.clone_from(&indent);
    } else {
        line.push_str(&" ".repeat(COLUMN - line.len()));
    }
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && line.len() + 1 + word.len() > WIDTH {
            out.push_str(&line);
            out.push('\n');
            line.clone_from(&indent);
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    out.push_str(&line);
    out.push('\n');
}
