//! The command line every KLF tool and generated driver shares (`KLF-CLI.md`).
//!
//! A command line is described as data: a tree of [`Command`]s, each with its
//! [`Arg`]s. One parser reads `argv` against the tree, one renderer writes help
//! and usage, and one generator writes shell completion scripts, so every
//! driver behaves and reads the same. A definition's commands reach a native
//! frontend through [`codec`].

pub mod codec;
mod complete;
mod help;
mod parse;

pub use complete::{Shell, completion_script};
pub use help::{help, usage};
pub use parse::{Error, ErrorKind, Matches, Outcome, Value, parse};

/// One command: the root of a tool or one of its subcommands.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Command {
    pub name: String,
    /// One line, shown in the parent's subcommand list and as the overview.
    pub abstract_text: String,
    /// Longer text shown under the overview.
    pub discussion: String,
    pub aliases: Vec<String>,
    pub args: Vec<Arg>,
    pub subcommands: Vec<Command>,
    /// The subcommand that runs when the first argument names none.
    pub default_subcommand: Option<String>,
    pub action: Action,
    pub hidden: bool,
    /// The version `--version` prints. Read from the root only.
    pub version: Option<String>,
}

/// What running a command does.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Action {
    /// A group: running it without a subcommand prints its help.
    #[default]
    None,
    /// Handled by the tool itself, keyed by name.
    Builtin(String),
    /// A definition's `ParsableCommand`, by its index in the definition.
    Definition(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgKind {
    /// `--name`, takes no value. A `Bool` flag is set, an `Int` flag counts.
    Flag,
    /// `--name value`, `--name=value`, `-n value` or `-nvalue`.
    Option,
    /// Taken by position.
    Positional,
    /// Everything after `--`.
    Rest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueType {
    Bool,
    Int,
    String,
    /// One of these names; the value is its index.
    Choice(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arity {
    /// Exactly one value; the last occurrence wins.
    One,
    /// Any number of values, in order.
    Many,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Arg {
    /// The declared property name; the default long name is its kebab case.
    pub id: String,
    pub kind: ArgKind,
    pub long: Option<String>,
    pub short: Option<char>,
    /// Further long names.
    pub aliases: Vec<String>,
    pub value: ValueType,
    pub arity: Arity,
    /// Must be given: an option or positional with no default.
    pub required: bool,
    /// The default as help shows it, when it can be shown.
    pub default: Option<String>,
    pub help: String,
    /// The placeholder in usage; empty means the long name or id.
    pub value_name: String,
    pub hidden: bool,
}

impl Arg {
    /// An argument of `kind` named after the property `id`.
    #[must_use]
    pub fn new(id: &str, kind: ArgKind, value: ValueType) -> Self {
        let long = matches!(kind, ArgKind::Flag | ArgKind::Option).then(|| kebab_case(id));
        Self {
            id: id.to_owned(),
            kind,
            long,
            short: None,
            aliases: Vec::new(),
            value,
            arity: Arity::One,
            required: false,
            default: None,
            help: String::new(),
            value_name: String::new(),
            hidden: false,
        }
    }

    #[must_use]
    pub fn short(mut self, short: char) -> Self {
        self.short = Some(short);
        self
    }

    #[must_use]
    pub fn help(mut self, help: &str) -> Self {
        help.clone_into(&mut self.help);
        self
    }

    #[must_use]
    pub fn many(mut self) -> Self {
        self.arity = Arity::Many;
        self
    }

    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    #[must_use]
    pub fn default_text(mut self, default: &str) -> Self {
        self.default = Some(default.to_owned());
        self
    }

    #[must_use]
    pub fn value_name(mut self, name: &str) -> Self {
        name.clone_into(&mut self.value_name);
        self
    }

    /// The placeholder usage shows for the value.
    #[must_use]
    pub fn placeholder(&self) -> String {
        if !self.value_name.is_empty() {
            return self.value_name.clone();
        }
        self.long.clone().unwrap_or_else(|| kebab_case(&self.id))
    }

    pub(crate) fn takes_value(&self) -> bool {
        self.kind != ArgKind::Flag
    }

    pub(crate) fn matches_long(&self, name: &str) -> bool {
        self.long.as_deref() == Some(name) || self.aliases.iter().any(|alias| alias == name)
    }
}

impl Command {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn about(mut self, text: &str) -> Self {
        text.clone_into(&mut self.abstract_text);
        self
    }

    #[must_use]
    pub fn arg(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    #[must_use]
    pub fn subcommand(mut self, command: Self) -> Self {
        self.subcommands.push(command);
        self
    }

    #[must_use]
    pub fn builtin(mut self, key: &str) -> Self {
        self.action = Action::Builtin(key.to_owned());
        self
    }

    /// The subcommand called `name`, or with that alias.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Self> {
        self.subcommands
            .iter()
            .find(|command| command.name == name || command.aliases.iter().any(|a| a == name))
    }

    /// The command reached by following `path` from here.
    #[must_use]
    pub fn at(&self, path: &[usize]) -> &Self {
        path.iter()
            .fold(self, |command, index| &command.subcommands[*index])
    }

    /// Checks what the parser relies on: unique names, short names that are
    /// one ASCII letter or digit, `Many` positionals last, and a default
    /// subcommand that exists.
    ///
    /// # Errors
    /// Returns the first problem found, naming the command.
    pub fn validate(&self) -> Result<(), String> {
        let mut longs = std::collections::HashSet::new();
        let mut shorts = std::collections::HashSet::new();
        let reserved = ["help", "version", "generate-completion-script"];
        for arg in &self.args {
            for name in arg.long.iter().chain(&arg.aliases) {
                if reserved.contains(&name.as_str()) || !longs.insert(name.clone()) {
                    return Err(format!(
                        "`{}`: option `--{name}` is reserved or repeated",
                        self.name
                    ));
                }
            }
            if let Some(short) = arg.short
                && (!short.is_ascii_alphanumeric() || short == 'h' || !shorts.insert(short))
            {
                return Err(format!(
                    "`{}`: option `-{short}` is reserved or repeated",
                    self.name
                ));
            }
        }
        let positionals: Vec<_> = self
            .args
            .iter()
            .filter(|arg| arg.kind == ArgKind::Positional)
            .collect();
        if let Some(position) = positionals.iter().position(|arg| arg.arity == Arity::Many)
            && position + 1 != positionals.len()
        {
            return Err(format!(
                "`{}`: a repeated argument must come last",
                self.name
            ));
        }
        let mut names = std::collections::HashSet::new();
        for command in &self.subcommands {
            for name in std::iter::once(&command.name).chain(&command.aliases) {
                if name == "help" || !names.insert(name.clone()) {
                    return Err(format!(
                        "`{}`: subcommand `{name}` is reserved or repeated",
                        self.name
                    ));
                }
            }
            command.validate()?;
        }
        if let Some(default) = &self.default_subcommand
            && self.find(default).is_none()
        {
            return Err(format!(
                "`{}`: default subcommand `{default}` does not exist",
                self.name
            ));
        }
        Ok(())
    }
}

/// `includeCounter` as `include-counter`, `HTMLOutput` as `html-output`.
#[must_use]
pub fn kebab_case(name: &str) -> String {
    let characters: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (index, character) in characters.iter().enumerate() {
        if *character == '_' {
            out.push('-');
            continue;
        }
        if character.is_ascii_uppercase() {
            let previous = index.checked_sub(1).map(|at| characters[at]);
            let next = characters.get(index + 1);
            let boundary = previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                || (previous.is_some_and(|p| p.is_ascii_uppercase())
                    && next.is_some_and(char::is_ascii_lowercase));
            if boundary && !out.ends_with('-') {
                out.push('-');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(*character);
        }
    }
    out
}

#[cfg(test)]
mod tests;
