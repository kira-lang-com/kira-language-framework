//! Shell completion scripts generated from the command tree.
//!
//! bash and zsh walk the words before the cursor to find the subcommand, then
//! offer that command's options and subcommands, the choices of an option
//! that takes one, and file names where a value is free text. fish states the
//! same per command with its seen-subcommand conditions.

use crate::{Arg, ArgKind, Command, ValueType};
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            _ => None,
        }
    }
}

/// The completion script for `root` in `shell`.
#[must_use]
pub fn completion_script(root: &Command, shell: Shell) -> String {
    let mut commands = Vec::new();
    collect(root, "", &mut commands);
    match shell {
        Shell::Bash => bash(root, &commands),
        Shell::Zsh => zsh(root, &commands),
        Shell::Fish => fish(root, &commands),
    }
}

/// Every visible command with its space-separated path from the root.
fn collect<'a>(command: &'a Command, path: &str, out: &mut Vec<(String, &'a Command)>) {
    out.push((path.to_owned(), command));
    for subcommand in command.subcommands.iter().filter(|c| !c.hidden) {
        let child = if path.is_empty() {
            subcommand.name.clone()
        } else {
            format!("{path} {}", subcommand.name)
        };
        collect(subcommand, &child, out);
    }
}

fn function_name(root: &Command) -> String {
    let name: String = root
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("_{name}")
}

fn spellings(arg: &Arg) -> Vec<String> {
    let mut names: Vec<String> = arg.short.iter().map(|s| format!("-{s}")).collect();
    names.extend(
        arg.long
            .iter()
            .chain(&arg.aliases)
            .map(|l| format!("--{l}")),
    );
    names
}

fn options(command: &Command) -> impl Iterator<Item = &Arg> {
    command
        .args
        .iter()
        .filter(|arg| !arg.hidden && matches!(arg.kind, ArgKind::Flag | ArgKind::Option))
}

fn takes_files(command: &Command) -> bool {
    command
        .args
        .iter()
        .any(|arg| arg.kind == ArgKind::Positional && arg.value == ValueType::String)
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// One `case` arm per subcommand name, moving `path` down the tree.
fn walk_arms(commands: &[(String, &Command)]) -> String {
    let mut out = String::new();
    for (path, command) in commands {
        for subcommand in command.subcommands.iter().filter(|c| !c.hidden) {
            let next = if path.is_empty() {
                subcommand.name.clone()
            } else {
                format!("{path} {}", subcommand.name)
            };
            let keys: Vec<String> = std::iter::once(&subcommand.name)
                .chain(&subcommand.aliases)
                .map(|name| quote(&format!("{path}:{name}")))
                .collect();
            let _ = writeln!(
                out,
                "            {}) path={} ;;",
                keys.join("|"),
                quote(&next)
            );
        }
    }
    out
}

fn bash(root: &Command, commands: &[(String, &Command)]) -> String {
    let function = function_name(root);
    let mut values = String::new();
    let mut words = String::new();
    for (path, command) in commands {
        for arg in options(command).filter(|arg| arg.kind == ArgKind::Option) {
            let keys: Vec<String> = spellings(arg)
                .iter()
                .map(|name| quote(&format!("{path}:{name}")))
                .collect();
            let reply = match &arg.value {
                ValueType::Choice(names) => {
                    format!(
                        "COMPREPLY=($(compgen -W {} -- \"$cur\"))",
                        quote(&names.join(" "))
                    )
                }
                ValueType::String => "COMPREPLY=($(compgen -f -- \"$cur\"))".to_owned(),
                _ => "COMPREPLY=()".to_owned(),
            };
            let _ = writeln!(values, "        {}) {reply}; return ;;", keys.join("|"));
        }
        let mut list: Vec<String> = options(command).flat_map(spellings).collect();
        list.push("--help".to_owned());
        list.extend(
            command
                .subcommands
                .iter()
                .filter(|c| !c.hidden)
                .map(|c| c.name.clone()),
        );
        let files = if takes_files(command) {
            " [[ $cur != -* ]] && COMPREPLY+=($(compgen -f -- \"$cur\"));"
        } else {
            ""
        };
        let _ = writeln!(
            words,
            "        {}) COMPREPLY=($(compgen -W {} -- \"$cur\"));{files} ;;",
            quote(path),
            quote(&list.join(" "))
        );
    }
    format!(
        "# bash completion for {name}\n\
         {function}() {{\n\
         \x20   local cur prev path i\n\
         \x20   cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
         \x20   prev=\"${{COMP_WORDS[COMP_CWORD-1]}}\"\n\
         \x20   path=''\n\
         \x20   for ((i = 1; i < COMP_CWORD; i++)); do\n\
         \x20       case \"$path:${{COMP_WORDS[i]}}\" in\n\
         {arms}\
         \x20       esac\n\
         \x20   done\n\
         \x20   case \"$path:$prev\" in\n\
         {values}\
         \x20   esac\n\
         \x20   case \"$path\" in\n\
         {words}\
         \x20   esac\n\
         }}\n\
         complete -F {function} {name}\n",
        name = root.name,
        arms = walk_arms(commands),
    )
}

fn zsh_item(name: &str, help: &str) -> String {
    quote(&format!(
        "{}:{}",
        name.replace(':', "\\:"),
        help.lines().next().unwrap_or("")
    ))
}

fn zsh(root: &Command, commands: &[(String, &Command)]) -> String {
    let function = function_name(root);
    let mut values = String::new();
    let mut words = String::new();
    for (path, command) in commands {
        for arg in options(command).filter(|arg| arg.kind == ArgKind::Option) {
            let keys: Vec<String> = spellings(arg)
                .iter()
                .map(|name| quote(&format!("{path}:{name}")))
                .collect();
            let reply = match &arg.value {
                ValueType::Choice(names) => format!(
                    "compadd -- {}",
                    names.iter().map(|n| quote(n)).collect::<Vec<_>>().join(" ")
                ),
                ValueType::String => "_files".to_owned(),
                _ => "_message value".to_owned(),
            };
            let _ = writeln!(values, "        {}) {reply}; return ;;", keys.join("|"));
        }
        let mut items: Vec<String> = options(command)
            .flat_map(|arg| {
                spellings(arg)
                    .into_iter()
                    .map(|name| zsh_item(&name, &arg.help))
            })
            .collect();
        items.push(zsh_item("--help", "Show help information."));
        let subcommands: Vec<String> = command
            .subcommands
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| zsh_item(&c.name, &c.abstract_text))
            .collect();
        let mut body = format!("items=({}); _describe option items", items.join(" "));
        if !subcommands.is_empty() {
            let _ = write!(
                body,
                "; commands=({}); _describe command commands",
                subcommands.join(" ")
            );
        }
        if takes_files(command) {
            body.push_str("; _files");
        }
        let _ = writeln!(words, "        {}) {body} ;;", quote(path));
    }
    format!(
        "#compdef {name}\n\
         {function}() {{\n\
         \x20   local path='' i prev=\"${{words[CURRENT-1]}}\"\n\
         \x20   local -a items commands\n\
         \x20   for ((i = 2; i < CURRENT; i++)); do\n\
         \x20       case \"$path:${{words[i]}}\" in\n\
         {arms}\
         \x20       esac\n\
         \x20   done\n\
         \x20   case \"$path:$prev\" in\n\
         {values}\
         \x20   esac\n\
         \x20   case \"$path\" in\n\
         {words}\
         \x20   esac\n\
         }}\n\
         {function} \"$@\"\n",
        name = root.name,
        arms = walk_arms(commands),
    )
}

fn fish(root: &Command, commands: &[(String, &Command)]) -> String {
    let mut out = format!("# fish completion for {}\n", root.name);
    let all: Vec<&str> = commands
        .iter()
        .flat_map(|(_, command)| command.subcommands.iter().map(|c| c.name.as_str()))
        .collect();
    for (path, command) in commands {
        let condition = if path.is_empty() {
            format!("not __fish_seen_subcommand_from {}", all.join(" "))
        } else {
            path.split(' ')
                .map(|name| format!("__fish_seen_subcommand_from {name}"))
                .collect::<Vec<_>>()
                .join("; and ")
        };
        for subcommand in command.subcommands.iter().filter(|c| !c.hidden) {
            let _ = writeln!(
                out,
                "complete -c {} -n {} -f -a {} -d {}",
                root.name,
                quote(&condition),
                quote(&subcommand.name),
                quote(&subcommand.abstract_text)
            );
        }
        for arg in options(command) {
            let mut line = format!("complete -c {} -n {}", root.name, quote(&condition));
            if let Some(short) = arg.short {
                let _ = write!(line, " -s {short}");
            }
            for long in arg.long.iter().chain(&arg.aliases) {
                let _ = write!(line, " -l {long}");
            }
            if arg.kind == ArgKind::Option {
                match &arg.value {
                    ValueType::Choice(names) => {
                        let _ = write!(line, " -x -a {}", quote(&names.join(" ")));
                    }
                    ValueType::String => line.push_str(" -r -F"),
                    _ => line.push_str(" -x"),
                }
            }
            if !arg.help.is_empty() {
                let _ = write!(line, " -d {}", quote(&arg.help));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}
