//! Reading `argv` against a [`Command`] tree.

use crate::{Action, Arg, ArgKind, Arity, Command, Shell, ValueType};
use std::fmt::Write as _;

/// One parsed value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    String(String),
    /// The index of the choice in its [`ValueType::Choice`].
    Choice(u32),
}

/// What the command line selected and what it gave each argument.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Matches {
    /// Subcommand indices from the root to the selected command.
    pub path: Vec<usize>,
    /// One entry per argument of the selected command, in declaration order.
    /// Empty means the argument was not given.
    pub values: Vec<Vec<Value>>,
}

impl Matches {
    #[must_use]
    pub fn command<'a>(&self, root: &'a Command) -> &'a Command {
        root.at(&self.path)
    }

    fn slot(&self, root: &Command, id: &str) -> Option<&[Value]> {
        let index = self
            .command(root)
            .args
            .iter()
            .position(|arg| arg.id == id)?;
        Some(&self.values[index])
    }

    #[must_use]
    pub fn present(&self, root: &Command, id: &str) -> bool {
        self.slot(root, id).is_some_and(|values| !values.is_empty())
    }

    #[must_use]
    pub fn bool(&self, root: &Command, id: &str) -> Option<bool> {
        match self.slot(root, id)?.last()? {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn int(&self, root: &Command, id: &str) -> Option<i64> {
        match self.slot(root, id)?.last()? {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn string(&self, root: &Command, id: &str) -> Option<String> {
        self.strings(root, id).pop()
    }

    #[must_use]
    pub fn strings(&self, root: &Command, id: &str) -> Vec<String> {
        let Some(values) = self.slot(root, id) else {
            return Vec::new();
        };
        let arg = self.command(root).args.iter().find(|arg| arg.id == id);
        values
            .iter()
            .filter_map(|value| match (value, arg.map(|arg| &arg.value)) {
                (Value::String(text), _) => Some(text.clone()),
                (Value::Choice(index), Some(ValueType::Choice(names))) => {
                    names.get(*index as usize).cloned()
                }
                _ => None,
            })
            .collect()
    }
}

/// What the command line asks the tool to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Run(Matches),
    /// Print the help of the command at this path.
    Help(Vec<usize>),
    Version,
    Completion(Shell),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    UnknownOption,
    MissingValue,
    InvalidValue,
    MissingArgument,
    UnexpectedArgument,
}

impl ErrorKind {
    /// The diagnostic code `KLF-CLI.md` section 5 gives this error.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownOption => "KLF-DRV-0040",
            Self::MissingValue => "KLF-DRV-0041",
            Self::InvalidValue => "KLF-DRV-0042",
            Self::MissingArgument => "KLF-DRV-0043",
            Self::UnexpectedArgument => "KLF-DRV-0044",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    /// The command whose usage the error is shown with.
    pub path: Vec<usize>,
}

impl Error {
    /// The error, the command's usage and where to find more, as a driver
    /// prints it after `<tool>: `.
    #[must_use]
    pub fn render(&self, root: &Command) -> String {
        let names = command_names(root, &self.path);
        format!(
            "error: {} [{}]\n{}\nSee '{} --help' for more information.",
            self.message,
            self.kind.code(),
            crate::usage(root, &self.path),
            names.join(" ")
        )
    }
}

pub(crate) fn command_names(root: &Command, path: &[usize]) -> Vec<String> {
    let mut names = vec![root.name.clone()];
    let mut command = root;
    for index in path {
        command = &command.subcommands[*index];
        names.push(command.name.clone());
    }
    names
}

/// Reads `arguments` (without `argv[0]`) against `root`.
///
/// # Errors
/// Returns an [`Error`] for an unknown option, a missing or invalid value, a
/// missing required argument, or an argument nothing takes.
pub fn parse(root: &Command, arguments: &[String]) -> Result<Outcome, Error> {
    if arguments.is_empty() && root.action == Action::None && !root.subcommands.is_empty() {
        return Ok(Outcome::Help(Vec::new()));
    }
    let mut parser = Parser {
        root,
        path: Vec::new(),
        command: root,
        values: vec![Vec::new(); root.args.len()],
        positional: 0,
        consumed: false,
        terminated: false,
    };
    let mut index = 0;
    while index < arguments.len() {
        let token = &arguments[index];
        index += 1;
        let mut next = || {
            let value = arguments.get(index).cloned();
            if value.is_some() {
                index += 1;
            }
            value
        };
        if let Some(outcome) = parser.token(token, &mut next)? {
            return Ok(outcome);
        }
    }
    parser.finish()
}

struct Parser<'a> {
    root: &'a Command,
    path: Vec<usize>,
    command: &'a Command,
    values: Vec<Vec<Value>>,
    /// How many positional values the command has taken.
    positional: usize,
    /// Whether the command has taken any argument yet.
    consumed: bool,
    terminated: bool,
}

impl Parser<'_> {
    fn error(&self, kind: ErrorKind, message: String) -> Error {
        Error {
            kind,
            message,
            path: self.path.clone(),
        }
    }

    fn descend(&mut self, index: usize) {
        self.path.push(index);
        self.command = &self.command.subcommands[index];
        self.values = vec![Vec::new(); self.command.args.len()];
        self.positional = 0;
        self.consumed = false;
    }

    /// Moves into the default subcommand when the command is a group that
    /// has one and has taken nothing itself.
    fn descend_default(&mut self) -> bool {
        if self.consumed || self.command.subcommands.is_empty() {
            return false;
        }
        let Some(default) = &self.command.default_subcommand else {
            return false;
        };
        let Some(index) = self
            .command
            .subcommands
            .iter()
            .position(|c| &c.name == default)
        else {
            return false;
        };
        self.descend(index);
        true
    }

    fn token(
        &mut self,
        token: &str,
        next: &mut dyn FnMut() -> Option<String>,
    ) -> Result<Option<Outcome>, Error> {
        if self.terminated {
            self.rest(token)?;
            return Ok(None);
        }
        match token {
            "--" => {
                if self.descend_default() {
                    return self.token(token, next);
                }
                self.terminated = true;
                return Ok(None);
            }
            "-h" | "--help" => return Ok(Some(Outcome::Help(self.path.clone()))),
            "--version" if self.root.version.is_some() && self.path.is_empty() => {
                return Ok(Some(Outcome::Version));
            }
            "--generate-completion-script" => {
                let name = next().unwrap_or_default();
                let shell = Shell::from_name(&name).ok_or_else(|| {
                    self.error(
                        ErrorKind::InvalidValue,
                        format!("unknown shell '{name}' (expected bash, zsh or fish)"),
                    )
                })?;
                return Ok(Some(Outcome::Completion(shell)));
            }
            _ => {}
        }
        if let Some(body) = token.strip_prefix("--") {
            return self.long(token, body, next).map(|()| None);
        }
        if token.len() > 1 && token.starts_with('-') && !is_number(token) {
            return self.short(token, next).map(|()| None);
        }
        self.positional_token(token, next)
    }

    fn long(
        &mut self,
        token: &str,
        body: &str,
        next: &mut dyn FnMut() -> Option<String>,
    ) -> Result<(), Error> {
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (body, None),
        };
        let found = self.command.args.iter().position(|arg| {
            matches!(arg.kind, ArgKind::Flag | ArgKind::Option) && arg.matches_long(name)
        });
        if let Some(slot) = found {
            self.consumed = true;
            return self.apply(slot, token, inline, next);
        }
        if let Some(positive) = name.strip_prefix("no-")
            && inline.is_none()
            && let Some(slot) = self.command.args.iter().position(|arg| {
                arg.kind == ArgKind::Flag
                    && arg.value == ValueType::Bool
                    && arg.matches_long(positive)
            })
        {
            self.consumed = true;
            self.values[slot].push(Value::Bool(false));
            return Ok(());
        }
        if self.descend_default() {
            return self.long(token, body, next);
        }
        let mut message = format!("unknown option '--{name}'");
        if let Some(suggestion) = self.suggest(name) {
            let _ = write!(message, "; did you mean '--{suggestion}'?");
        }
        Err(self.error(ErrorKind::UnknownOption, message))
    }

    fn short(
        &mut self,
        token: &str,
        next: &mut dyn FnMut() -> Option<String>,
    ) -> Result<(), Error> {
        let letters: Vec<char> = token[1..].chars().collect();
        let mut at = 0;
        while at < letters.len() {
            let letter = letters[at];
            let Some(slot) = self
                .command
                .args
                .iter()
                .position(|arg| arg.short == Some(letter))
            else {
                if at == 0 && self.descend_default() {
                    return self.short(token, next);
                }
                return Err(self.error(
                    ErrorKind::UnknownOption,
                    format!("unknown option '-{letter}'"),
                ));
            };
            self.consumed = true;
            if self.command.args[slot].takes_value() {
                let attached: String = letters[at + 1..].iter().collect();
                let attached = attached
                    .strip_prefix('=')
                    .map_or(attached.clone(), str::to_owned);
                let inline = (!attached.is_empty()).then_some(attached);
                return self.apply(slot, &format!("-{letter}"), inline, next);
            }
            self.apply(slot, &format!("-{letter}"), None, next)?;
            at += 1;
        }
        Ok(())
    }

    /// Records one occurrence of the flag or option in `slot`.
    fn apply(
        &mut self,
        slot: usize,
        spelling: &str,
        inline: Option<String>,
        next: &mut dyn FnMut() -> Option<String>,
    ) -> Result<(), Error> {
        let arg = &self.command.args[slot];
        if arg.kind == ArgKind::Flag {
            if inline.is_some() {
                return Err(self.error(
                    ErrorKind::InvalidValue,
                    format!("flag '{spelling}' does not take a value"),
                ));
            }
            match arg.value {
                ValueType::Int => {
                    let count = match self.values[slot].last() {
                        Some(Value::Int(count)) => *count,
                        _ => 0,
                    };
                    self.values[slot] = vec![Value::Int(count + 1)];
                }
                _ => self.values[slot].push(Value::Bool(true)),
            }
            return Ok(());
        }
        let Some(raw) = inline.or_else(next) else {
            return Err(self.error(
                ErrorKind::MissingValue,
                format!("missing value for '{spelling} <{}>'", arg.placeholder()),
            ));
        };
        let value = self.convert(arg, spelling, &raw)?;
        if arg.arity == Arity::One {
            self.values[slot].clear();
        }
        self.values[slot].push(value);
        Ok(())
    }

    fn convert(&self, arg: &Arg, spelling: &str, raw: &str) -> Result<Value, Error> {
        let invalid = |expected: String| {
            self.error(
                ErrorKind::InvalidValue,
                format!("the value '{raw}' is invalid for '{spelling}'; expected {expected}"),
            )
        };
        match &arg.value {
            ValueType::String => Ok(Value::String(raw.to_owned())),
            ValueType::Int => raw
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| invalid("an integer".to_owned())),
            ValueType::Bool => match raw {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(invalid("true or false".to_owned())),
            },
            ValueType::Choice(names) => names
                .iter()
                .position(|name| name == raw)
                .map(|index| Value::Choice(u32::try_from(index).unwrap_or(u32::MAX)))
                .ok_or_else(|| invalid(format!("one of {}", names.join(", ")))),
        }
    }

    fn positional_token(
        &mut self,
        token: &str,
        next: &mut dyn FnMut() -> Option<String>,
    ) -> Result<Option<Outcome>, Error> {
        if !self.consumed && !self.command.subcommands.is_empty() {
            if token == "help" {
                return Ok(Some(Outcome::Help(self.help_path(next))));
            }
            if let Some(index) = self
                .command
                .subcommands
                .iter()
                .position(|c| c.name == token || c.aliases.iter().any(|a| a == token))
            {
                self.descend(index);
                return Ok(None);
            }
            if self.descend_default() {
                return self.positional_token(token, next);
            }
        }
        let positionals: Vec<usize> = self
            .command
            .args
            .iter()
            .enumerate()
            .filter(|(_, arg)| arg.kind == ArgKind::Positional)
            .map(|(index, _)| index)
            .collect();
        let slot = positionals
            .get(self.positional)
            .or_else(|| {
                positionals
                    .last()
                    .filter(|slot| self.command.args[**slot].arity == Arity::Many)
            })
            .copied();
        let Some(slot) = slot else {
            let mut message = format!("unexpected argument '{token}'");
            if !self.command.subcommands.is_empty()
                && let Some(suggestion) = self.suggest_subcommand(token)
            {
                let _ = write!(message, "; did you mean '{suggestion}'?");
            }
            return Err(self.error(ErrorKind::UnexpectedArgument, message));
        };
        let arg = &self.command.args[slot];
        let value = self.convert(arg, &format!("<{}>", arg.placeholder()), token)?;
        self.values[slot].push(value);
        if arg.arity == Arity::One {
            self.positional += 1;
        }
        self.consumed = true;
        Ok(None)
    }

    fn rest(&mut self, token: &str) -> Result<(), Error> {
        if let Some(slot) = self
            .command
            .args
            .iter()
            .position(|arg| arg.kind == ArgKind::Rest)
        {
            self.values[slot].push(Value::String(token.to_owned()));
            return Ok(());
        }
        let mut never = || None;
        self.positional_token(token, &mut never).map(|_| ())
    }

    /// The command `help a b` names, as far as its words name subcommands.
    fn help_path(&self, next: &mut dyn FnMut() -> Option<String>) -> Vec<usize> {
        let mut path = self.path.clone();
        let mut command = self.command;
        while let Some(word) = next() {
            let Some(index) = command
                .subcommands
                .iter()
                .position(|c| c.name == word || c.aliases.contains(&word))
            else {
                break;
            };
            path.push(index);
            command = &command.subcommands[index];
        }
        path
    }

    fn finish(mut self) -> Result<Outcome, Error> {
        if self.command.action == Action::None && !self.command.subcommands.is_empty() {
            let default = self
                .command
                .default_subcommand
                .as_ref()
                .and_then(|default| {
                    self.command
                        .subcommands
                        .iter()
                        .position(|c| &c.name == default)
                });
            let Some(index) = default else {
                return Ok(Outcome::Help(self.path));
            };
            self.descend(index);
            return self.finish();
        }
        for (slot, arg) in self.command.args.iter().enumerate() {
            if arg.required && self.values[slot].is_empty() {
                let shown = match arg.kind {
                    ArgKind::Positional | ArgKind::Rest => format!("<{}>", arg.placeholder()),
                    _ => format!(
                        "--{} <{}>",
                        arg.long.clone().unwrap_or_default(),
                        arg.placeholder()
                    ),
                };
                return Err(self.error(
                    ErrorKind::MissingArgument,
                    format!("missing expected argument '{shown}'"),
                ));
            }
        }
        Ok(Outcome::Run(Matches {
            path: self.path,
            values: self.values,
        }))
    }

    fn suggest(&self, name: &str) -> Option<String> {
        closest(
            name,
            self.command
                .args
                .iter()
                .filter(|arg| !arg.hidden)
                .flat_map(|arg| arg.long.iter().chain(&arg.aliases))
                .map(String::as_str)
                .chain(["help"]),
        )
    }

    fn suggest_subcommand(&self, name: &str) -> Option<String> {
        closest(
            name,
            self.command
                .subcommands
                .iter()
                .filter(|c| !c.hidden)
                .map(|c| c.name.as_str()),
        )
    }
}

fn is_number(token: &str) -> bool {
    token[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The candidate nearest `name` by edit distance, when it is near enough to
/// be a likely typo.
fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    candidates
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, candidate)| *distance <= (candidate.len() / 3).max(1))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.to_owned())
}

/// Edit distance counting a swap of two neighbours as one edit.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut table = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in table.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in table[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (table[i - 1][j] + 1)
                .min(table[i][j - 1] + 1)
                .min(table[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(table[i - 2][j - 2] + 1);
            }
            table[i][j] = best;
        }
    }
    table[a.len()][b.len()]
}
