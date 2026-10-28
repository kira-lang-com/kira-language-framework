//! The directives of C17 6.10.1 to 6.10.5 as the preprocessor runs them:
//! conditionals, `#define`, `#undef`, `#include`, and `#error`.

use std::path::{Path, PathBuf};

use crate::preprocess::cond::truth;
use crate::preprocess::expand::{expand, parse_define, render};
use crate::preprocess::lex::tokenize;
use crate::preprocess::{Branches, Condition, HeaderError, State};

const MAX_INCLUDE_DEPTH: usize = 200;

impl State<'_, '_> {
    /// Runs the directive `text`, the line after its `#`.
    pub(crate) fn directive(
        &mut self,
        line: usize,
        text: &str,
        conditions: &mut Vec<Condition>,
    ) -> Result<(), HeaderError> {
        let text = text.trim();
        let name_end = text
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        let (name, args) = (&text[..name_end], text[name_end..].trim());
        let active = conditions.last().is_none_or(|condition| condition.active);
        match name {
            "" | "pragma" | "warning" | "line" => Ok(()),
            "if" | "ifdef" | "ifndef" => {
                let parent = active;
                let value = if parent {
                    self.condition(name, args, line)?
                } else {
                    false
                };
                conditions.push(Condition {
                    line,
                    parent,
                    active: parent && value,
                    branches: Branches::Open {
                        taken: !parent || value,
                    },
                });
                Ok(())
            }
            "elif" => {
                let top = *conditions
                    .last()
                    .ok_or_else(|| self.error(line, "`#elif` without `#if`"))?;
                let Branches::Open { taken } = top.branches else {
                    return Err(self.error(line, "`#elif` after `#else`"));
                };
                let value = if top.parent && !taken {
                    self.truth_of(args, line)?
                } else {
                    false
                };
                if let Some(last) = conditions.last_mut() {
                    last.active = top.parent && !taken && value;
                    last.branches = Branches::Open {
                        taken: taken || value,
                    };
                }
                Ok(())
            }
            "else" => {
                let top = *conditions
                    .last()
                    .ok_or_else(|| self.error(line, "`#else` without `#if`"))?;
                let Branches::Open { taken } = top.branches else {
                    return Err(self.error(line, "`#else` after `#else`"));
                };
                if let Some(last) = conditions.last_mut() {
                    last.active = top.parent && !taken;
                    last.branches = Branches::Else;
                }
                Ok(())
            }
            "endif" => {
                conditions
                    .pop()
                    .ok_or_else(|| self.error(line, "`#endif` without `#if`"))?;
                Ok(())
            }
            _ if !active => Ok(()),
            "define" => self.define(line, args),
            "undef" => {
                let name = self.identifier(args, line, "undef")?;
                self.macros.remove(&name);
                self.defined.retain(|defined| *defined != name);
                Ok(())
            }
            "include" => self.include(line, args),
            "error" => Err(self.error(line, format!("#error {args}"))),
            other => Err(self.error(line, format!("unknown directive `#{other}`"))),
        }
    }

    fn condition(&self, directive: &str, args: &str, line: usize) -> Result<bool, HeaderError> {
        if directive == "if" {
            return self.truth_of(args, line);
        }
        let name = self.identifier(args, line, directive)?;
        let defined = self.macros.contains_key(&name);
        Ok(if directive == "ifdef" { defined } else { !defined })
    }

    fn truth_of(&self, args: &str, line: usize) -> Result<bool, HeaderError> {
        let tokens = tokenize(args, line).map_err(|fault| self.fault(fault))?;
        truth(&tokens, &self.macros, line).map_err(|fault| self.fault(fault))
    }

    fn identifier(&self, args: &str, line: usize, directive: &str) -> Result<String, HeaderError> {
        let name: String = args
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(self.error(line, format!("`#{directive}` needs a macro name")));
        }
        Ok(name)
    }

    fn define(&mut self, line: usize, args: &str) -> Result<(), HeaderError> {
        let (name, mut definition) = parse_define(args, line).map_err(|fault| self.fault(fault))?;
        if definition.params.is_none() {
            let expanded = expand(definition.body.clone(), &self.macros)
                .map_err(|fault| self.fault(fault))?;
            definition.value = render(&expanded);
        }
        if !self.defined.contains(&name) {
            self.defined.push(name.clone());
        }
        self.macros.insert(name, definition);
        Ok(())
    }

    fn include(&mut self, line: usize, args: &str) -> Result<(), HeaderError> {
        let (name, quoted) = self.header_name(args, line)?;
        let Some((path, text)) = self.locate(&name, quoted) else {
            let delimited = if quoted {
                format!("\"{name}\"")
            } else {
                format!("<{name}>")
            };
            self.remarks.push(format!(
                "{}:{line}: `#include {delimited}` was not found and is left out",
                self.current().display()
            ));
            return Ok(());
        };
        if self.includes.contains(&path) {
            self.remarks.push(format!(
                "{}:{line}: `#include {name}` closes an include cycle and is left out",
                self.current().display()
            ));
            return Ok(());
        }
        if self.includes.len() >= MAX_INCLUDE_DEPTH {
            return Err(self.error(line, "`#include` nested too deeply"));
        }
        self.file(&path, &text)
    }

    /// The name between the delimiters of an `#include`, and whether it used
    /// quotes. A macro that expands to a header name is accepted too.
    fn header_name(&self, args: &str, line: usize) -> Result<(String, bool), HeaderError> {
        if let Some(rest) = args.strip_prefix('"') {
            return rest
                .find('"')
                .map(|end| (rest[..end].to_owned(), true))
                .ok_or_else(|| self.error(line, "unterminated header name in `#include`"));
        }
        if let Some(rest) = args.strip_prefix('<') {
            return rest
                .find('>')
                .map(|end| (rest[..end].to_owned(), false))
                .ok_or_else(|| self.error(line, "unterminated header name in `#include`"));
        }
        let tokens = tokenize(args, line).map_err(|fault| self.fault(fault))?;
        let expanded = expand(tokens, &self.macros).map_err(|fault| self.fault(fault))?;
        let text = render(&expanded);
        if text.starts_with('"') || text.starts_with('<') {
            return self.header_name(&text, line);
        }
        Err(self.error(line, "`#include` expects a header name"))
    }

    /// The first candidate that the reader returns: beside the including file
    /// for a quoted name, then along the include directories.
    fn locate(&mut self, name: &str, quoted: bool) -> Option<(PathBuf, String)> {
        let mut candidates = Vec::new();
        if quoted
            && let Some(directory) = self.includes.last().and_then(|path| path.parent())
        {
            candidates.push(directory.join(name));
        }
        candidates.extend(
            self.environment
                .include_dirs
                .iter()
                .map(|directory| directory.join(name)),
        );
        candidates
            .into_iter()
            .find_map(|path| (self.read)(&path).map(|text| (path, text)))
    }

    pub(crate) fn current(&self) -> PathBuf {
        self.includes
            .last()
            .map_or_else(|| Path::new("<header>").to_path_buf(), Clone::clone)
    }
}
