//! The controlling expression of `#if` and `#elif` (C17 6.10.1): `defined`,
//! macro expansion, then integer arithmetic in the C operator set. Identifiers
//! left after expansion read as 0, as the standard requires.

use crate::preprocess::expand::{Macros, expand};
use crate::preprocess::lex::{Fault, Kind, Token, integer_literal};

/// Whether the controlling expression `tokens` is non-zero.
pub(crate) fn truth(tokens: &[Token], macros: &Macros, line: usize) -> Result<bool, Fault> {
    let replaced = replace_defined(tokens, macros, line)?;
    let expanded = expand(replaced, macros)?;
    let mut parser = Parser {
        tokens: expanded,
        position: 0,
        line,
    };
    if parser.tokens.is_empty() {
        return Err(Fault::new(line, "`#if` needs an expression"));
    }
    let value = parser.conditional()?;
    if let Some(token) = parser.tokens.get(parser.position) {
        return Err(Fault::new(
            line,
            format!("unexpected `{}` in the expression", token.text),
        ));
    }
    Ok(value != 0)
}

fn replace_defined(tokens: &[Token], macros: &Macros, line: usize) -> Result<Vec<Token>, Fault> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if token.kind != Kind::Ident || token.text != "defined" {
            out.push(token.clone());
            index += 1;
            continue;
        }
        let (name, consumed) = if tokens.get(index + 1).is_some_and(|t| t.is("(")) {
            match (tokens.get(index + 2), tokens.get(index + 3)) {
                (Some(name), Some(close)) if name.kind == Kind::Ident && close.is(")") => {
                    (name, 4)
                }
                _ => {
                    return Err(Fault::new(line, "`defined(` needs a macro name and `)`"));
                }
            }
        } else {
            match tokens.get(index + 1) {
                Some(name) if name.kind == Kind::Ident => (name, 2),
                _ => return Err(Fault::new(line, "`defined` needs a macro name")),
            }
        };
        let value = if macros.contains_key(&name.text) { "1" } else { "0" };
        out.push(Token::new(Kind::Number, value, line));
        index += consumed;
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    line: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn eat(&mut self, punct: &str) -> bool {
        if self.peek().is_some_and(|t| t.is(punct)) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn conditional(&mut self) -> Result<i64, Fault> {
        let condition = self.binary(1)?;
        if !self.eat("?") {
            return Ok(condition);
        }
        let when_true = self.conditional()?;
        if !self.eat(":") {
            return Err(Fault::new(self.line, "`?` needs a matching `:`"));
        }
        let when_false = self.conditional()?;
        Ok(if condition != 0 { when_true } else { when_false })
    }

    fn binary(&mut self, minimum: u8) -> Result<i64, Fault> {
        let mut left = self.unary()?;
        while let Some(precedence) = self.peek().and_then(|t| binary_precedence(&t.text)) {
            if precedence < minimum {
                break;
            }
            let op = self.tokens[self.position].text.clone();
            self.position += 1;
            let right = self.binary(precedence + 1)?;
            left = apply(&op, left, right, self.line)?;
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<i64, Fault> {
        let Some(token) = self.peek().cloned() else {
            return Err(Fault::new(self.line, "the expression ends too early"));
        };
        self.position += 1;
        match (token.kind, token.text.as_str()) {
            (Kind::Punct, "+") => self.unary(),
            (Kind::Punct, "-") => Ok(self.unary()?.wrapping_neg()),
            (Kind::Punct, "~") => Ok(!self.unary()?),
            (Kind::Punct, "!") => Ok(i64::from(self.unary()? == 0)),
            (Kind::Punct, "(") => {
                let value = self.conditional()?;
                if !self.eat(")") {
                    return Err(Fault::new(self.line, "missing `)` in the expression"));
                }
                Ok(value)
            }
            (Kind::Number, text) => integer_literal(text)
                .ok_or_else(|| {
                    Fault::new(self.line, format!("`{text}` is not an integer constant"))
                }),
            (Kind::Ident, _) => Ok(0),
            (Kind::Literal, text) if text.starts_with('\'') => character(text, self.line),
            _ => Err(Fault::new(
                self.line,
                format!("`{}` cannot appear in an expression", token.text),
            )),
        }
    }
}

fn binary_precedence(text: &str) -> Option<u8> {
    Some(match text {
        "||" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        _ => return None,
    })
}

fn apply(op: &str, left: i64, right: i64, line: usize) -> Result<i64, Fault> {
    let truth = |condition: bool| i64::from(condition);
    Ok(match op {
        "||" => truth(left != 0 || right != 0),
        "&&" => truth(left != 0 && right != 0),
        "|" => left | right,
        "^" => left ^ right,
        "&" => left & right,
        "==" => truth(left == right),
        "!=" => truth(left != right),
        "<" => truth(left < right),
        ">" => truth(left > right),
        "<=" => truth(left <= right),
        ">=" => truth(left >= right),
        "<<" => shift_left(left, right),
        ">>" => shift_right(left, right),
        "+" => left.wrapping_add(right),
        "-" => left.wrapping_sub(right),
        "*" => left.wrapping_mul(right),
        "/" | "%" if right == 0 => {
            return Err(Fault::new(line, "division by zero in the expression"));
        }
        "/" => left.wrapping_div(right),
        _ => left.wrapping_rem(right),
    })
}

fn shift_left(value: i64, amount: i64) -> i64 {
    u32::try_from(amount)
        .ok()
        .and_then(|amount| value.checked_shl(amount))
        .unwrap_or(0)
}

fn shift_right(value: i64, amount: i64) -> i64 {
    u32::try_from(amount)
        .ok()
        .and_then(|amount| value.checked_shr(amount))
        .unwrap_or(if value < 0 { -1 } else { 0 })
}

fn character(text: &str, line: usize) -> Result<i64, Fault> {
    let inner = text
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_default();
    let value = match inner {
        "\\n" => Some('\n'),
        "\\t" => Some('\t'),
        "\\0" => Some('\0'),
        "\\\\" => Some('\\'),
        "\\'" => Some('\''),
        "\\\"" => Some('"'),
        _ => {
            let mut chars = inner.chars();
            match (chars.next(), chars.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    };
    value
        .map(|ch| i64::from(u32::from(ch)))
        .ok_or_else(|| Fault::new(line, format!("unsupported character constant `{text}`")))
}
