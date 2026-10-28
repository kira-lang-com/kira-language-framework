//! A command tree as text, for the descriptor blob a native frontend carries.
//!
//! One record per line, fields separated by tabs, `\\`, `\t` and `\n` escaped.
//! A `C` record opens a command at a depth; the `A` records after it are its
//! arguments. Commands follow their parent in depth-first order.

use crate::{Action, Arg, ArgKind, Arity, Command, ValueType};

#[must_use]
pub fn encode(command: &Command) -> String {
    let mut out = String::new();
    encode_command(command, 0, &mut out);
    out
}

fn encode_command(command: &Command, depth: usize, out: &mut String) {
    let action = match &command.action {
        Action::None => String::new(),
        Action::Builtin(key) => format!("b:{key}"),
        Action::Definition(index) => format!("d:{index}"),
    };
    let mut fields = vec![
        "C".to_owned(),
        depth.to_string(),
        command.name.clone(),
        command.abstract_text.clone(),
        command.discussion.clone(),
        command.default_subcommand.clone().unwrap_or_default(),
        action,
        flag(command.hidden),
        flag(command.version.is_some()),
        command.version.clone().unwrap_or_default(),
    ];
    fields.extend(command.aliases.iter().cloned());
    line(out, &fields);
    for arg in &command.args {
        let (value, choices) = match &arg.value {
            ValueType::Bool => ("b", &[][..]),
            ValueType::Int => ("i", &[][..]),
            ValueType::String => ("s", &[][..]),
            ValueType::Choice(names) => ("c", names.as_slice()),
        };
        let mut fields = vec![
            "A".to_owned(),
            arg.id.clone(),
            match arg.kind {
                ArgKind::Flag => "f",
                ArgKind::Option => "o",
                ArgKind::Positional => "p",
                ArgKind::Rest => "r",
            }
            .to_owned(),
            arg.long.clone().unwrap_or_default(),
            arg.short.map(String::from).unwrap_or_default(),
            value.to_owned(),
            if arg.arity == Arity::Many { "n" } else { "1" }.to_owned(),
            flag(arg.required),
            flag(arg.default.is_some()),
            arg.default.clone().unwrap_or_default(),
            arg.help.clone(),
            arg.value_name.clone(),
            flag(arg.hidden),
            arg.aliases.len().to_string(),
        ];
        fields.extend(arg.aliases.iter().cloned());
        fields.extend(choices.iter().cloned());
        line(out, &fields);
    }
    for subcommand in &command.subcommands {
        encode_command(subcommand, depth + 1, out);
    }
}

fn flag(value: bool) -> String {
    if value { "1" } else { "0" }.to_owned()
}

fn line(out: &mut String, fields: &[String]) {
    let escaped: Vec<String> = fields.iter().map(|field| escape(field)).collect();
    out.push_str(&escaped.join("\t"));
    out.push('\n');
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Reads back what [`encode`] wrote; `None` when the text is malformed.
#[must_use]
pub fn decode(text: &str) -> Option<Command> {
    let mut stack: Vec<Command> = Vec::new();
    for record in text.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<String> = record.split('\t').map(unescape).collect();
        match fields.first()?.as_str() {
            "C" => {
                let depth: usize = fields.get(1)?.parse().ok()?;
                while stack.len() > depth {
                    let done = stack.pop()?;
                    stack.last_mut()?.subcommands.push(done);
                }
                if stack.len() != depth {
                    return None;
                }
                stack.push(decode_command(&fields)?);
            }
            "A" => {
                let arg = decode_arg(&fields)?;
                stack.last_mut()?.args.push(arg);
            }
            _ => return None,
        }
    }
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.subcommands.push(done);
    }
    stack.pop()
}

fn decode_command(fields: &[String]) -> Option<Command> {
    let action = match fields.get(6)?.split_once(':') {
        None => Action::None,
        Some(("b", key)) => Action::Builtin(key.to_owned()),
        Some(("d", index)) => Action::Definition(index.parse().ok()?),
        Some(_) => return None,
    };
    let default = fields.get(5)?;
    Some(Command {
        name: fields.get(2)?.clone(),
        abstract_text: fields.get(3)?.clone(),
        discussion: fields.get(4)?.clone(),
        default_subcommand: (!default.is_empty()).then(|| default.clone()),
        action,
        hidden: fields.get(7)? == "1",
        version: (fields.get(8)? == "1").then(|| fields.get(9).cloned().unwrap_or_default()),
        aliases: fields.get(10..).unwrap_or_default().to_vec(),
        args: Vec::new(),
        subcommands: Vec::new(),
    })
}

fn decode_arg(fields: &[String]) -> Option<Arg> {
    let kind = match fields.get(2)?.as_str() {
        "f" => ArgKind::Flag,
        "o" => ArgKind::Option,
        "p" => ArgKind::Positional,
        "r" => ArgKind::Rest,
        _ => return None,
    };
    let alias_count: usize = fields.get(13)?.parse().ok()?;
    let aliases = fields.get(14..14 + alias_count)?.to_vec();
    let value = match fields.get(5)?.as_str() {
        "b" => ValueType::Bool,
        "i" => ValueType::Int,
        "s" => ValueType::String,
        "c" => ValueType::Choice(fields.get(14 + alias_count..)?.to_vec()),
        _ => return None,
    };
    let long = fields.get(3)?;
    Some(Arg {
        id: fields.get(1)?.clone(),
        kind,
        long: (!long.is_empty()).then(|| long.clone()),
        short: fields.get(4)?.chars().next(),
        aliases,
        value,
        arity: if fields.get(6)? == "n" {
            Arity::Many
        } else {
            Arity::One
        },
        required: fields.get(7)? == "1",
        default: (fields.get(8)? == "1").then(|| fields.get(9).cloned().unwrap_or_default()),
        help: fields.get(10)?.clone(),
        value_name: fields.get(11)?.clone(),
        hidden: fields.get(12)? == "1",
    })
}
