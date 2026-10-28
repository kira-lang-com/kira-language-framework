//! `#define` bodies and macro expansion (C17 6.10.3): object-like and
//! function-like macros, variadic parameters, and the `#` and `##` operators.
//! Expansion rescans each replacement together with the tokens after it, and
//! a token never expands to a macro that produced it (its hide set).

use std::collections::{HashMap, VecDeque};

use crate::preprocess::lex::{Fault, Kind, Token, tokenize};

const VA_ARGS: &str = "__VA_ARGS__";

#[derive(Clone, Debug)]
pub(crate) struct Macro {
    pub(crate) params: Option<Vec<String>>,
    pub(crate) variadic: bool,
    pub(crate) body: Vec<Token>,
    /// The expanded replacement of an object-like macro, rendered as text.
    pub(crate) value: String,
}

pub(crate) type Macros = HashMap<String, Macro>;

/// The name and definition a `#define` line (without `define`) declares.
pub(crate) fn parse_define(text: &str, line: usize) -> Result<(String, Macro), Fault> {
    let text = text.trim_start();
    let name_end = ident_len(text);
    if name_end == 0 {
        return Err(Fault::new(line, "`#define` needs a macro name"));
    }
    let name = text[..name_end].to_owned();
    let rest = &text[name_end..];
    if let Some(list) = rest.strip_prefix('(') {
        let close = list
            .find(')')
            .ok_or_else(|| Fault::new(line, format!("missing `)` in the parameters of `{name}`")))?;
        let (params, variadic) = parse_params(&list[..close], &name, line)?;
        let body = tokenize(&list[close + 1..], line)?;
        check_body(&body, &params, &name, line)?;
        return Ok((
            name,
            Macro {
                params: Some(params),
                variadic,
                body,
                value: String::new(),
            },
        ));
    }
    let body = tokenize(rest, line)?;
    Ok((
        name,
        Macro {
            params: None,
            variadic: false,
            body,
            value: String::new(),
        },
    ))
}

fn ident_len(text: &str) -> usize {
    let mut len = 0;
    for (index, c) in text.char_indices() {
        let valid = if index == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        };
        if !valid {
            break;
        }
        len = index + c.len_utf8();
    }
    len
}

fn parse_params(list: &str, name: &str, line: usize) -> Result<(Vec<String>, bool), Fault> {
    let mut params: Vec<String> = Vec::new();
    let mut variadic = false;
    let list = list.trim();
    if list.is_empty() {
        return Ok((params, variadic));
    }
    for raw in list.split(',') {
        if variadic {
            return Err(Fault::new(line, format!("`...` must be the last parameter of `{name}`")));
        }
        let raw = raw.trim();
        if raw == "..." {
            variadic = true;
            params.push(VA_ARGS.to_owned());
            continue;
        }
        if ident_len(raw) != raw.len() || raw.is_empty() {
            return Err(Fault::new(
                line,
                format!("`{raw}` is not a parameter name of `{name}`"),
            ));
        }
        if params.iter().any(|p| p == raw) {
            return Err(Fault::new(
                line,
                format!("duplicate parameter `{raw}` in `{name}`"),
            ));
        }
        params.push(raw.to_owned());
    }
    Ok((params, variadic))
}

fn check_body(body: &[Token], params: &[String], name: &str, line: usize) -> Result<(), Fault> {
    for (index, token) in body.iter().enumerate() {
        if token.is("#") {
            let names_param = body
                .get(index + 1)
                .is_some_and(|next| next.kind == Kind::Ident && params.contains(&next.text));
            if !names_param {
                return Err(Fault::new(
                    line,
                    format!("`#` in `{name}` is not followed by a parameter"),
                ));
            }
        }
    }
    let first_or_last_paste = body.first().is_some_and(|t| t.is("##"))
        || body.last().is_some_and(|t| t.is("##"));
    if first_or_last_paste {
        return Err(Fault::new(line, format!("`##` at an end of `{name}`")));
    }
    Ok(())
}

/// Expands every macro in `tokens`, rescanning as C requires.
pub(crate) fn expand(tokens: Vec<Token>, macros: &Macros) -> Result<Vec<Token>, Fault> {
    let mut input: VecDeque<Token> = tokens.into();
    let mut out = Vec::new();
    while let Some(token) = input.pop_front() {
        let Some(definition) = macros
            .get(&token.text)
            .filter(|_| token.kind == Kind::Ident && !token.hide.contains(&token.text))
        else {
            out.push(token);
            continue;
        };
        let mut hide = token.hide.clone();
        hide.push(token.text.clone());
        let mut replacement = match &definition.params {
            None => substitute(&definition.body, None, macros, &hide, token.line)?,
            Some(params) => {
                if !input.front().is_some_and(|next| next.is("(")) {
                    out.push(token);
                    continue;
                }
                input.pop_front();
                let arguments = collect_arguments(&mut input, &token)?;
                let bound = bind(params, definition.variadic, arguments, &token)?;
                substitute(
                    &definition.body,
                    Some((params, &bound)),
                    macros,
                    &hide,
                    token.line,
                )?
            }
        };
        if let Some(first) = replacement.first_mut() {
            first.spaced = token.spaced;
        }
        for replaced in replacement.into_iter().rev() {
            input.push_front(replaced);
        }
    }
    Ok(out)
}

fn collect_arguments(
    input: &mut VecDeque<Token>,
    name: &Token,
) -> Result<Vec<Vec<Token>>, Fault> {
    let mut arguments = vec![Vec::new()];
    let mut depth = 0usize;
    loop {
        let Some(token) = input.pop_front() else {
            return Err(Fault::new(
                name.line,
                format!("unterminated argument list invoking `{}`", name.text),
            ));
        };
        if token.is("(") {
            depth += 1;
        } else if token.is(")") {
            if depth == 0 {
                break;
            }
            depth -= 1;
        } else if token.is(",") && depth == 0 {
            arguments.push(Vec::new());
            continue;
        }
        if let Some(current) = arguments.last_mut() {
            current.push(token);
        }
    }
    if arguments.len() == 1 && arguments[0].is_empty() {
        arguments.clear();
    }
    Ok(arguments)
}

/// The arguments one per parameter; a variadic parameter takes every argument
/// from its position on, commas included.
fn bind(
    params: &[String],
    variadic: bool,
    arguments: Vec<Vec<Token>>,
    name: &Token,
) -> Result<Vec<Vec<Token>>, Fault> {
    let fixed = if variadic { params.len() - 1 } else { params.len() };
    let count_ok = if variadic {
        arguments.len() >= fixed
    } else {
        arguments.len() == fixed
    };
    if !count_ok {
        return Err(Fault::new(
            name.line,
            format!(
                "`{}` takes {} argument(s), but {} were given",
                name.text,
                fixed,
                arguments.len()
            ),
        ));
    }
    if !variadic {
        return Ok(arguments);
    }
    let mut bound: Vec<Vec<Token>> = arguments.iter().take(fixed).cloned().collect();
    let mut rest = Vec::new();
    for (index, argument) in arguments.into_iter().skip(fixed).enumerate() {
        if index > 0 {
            rest.push(Token::new(Kind::Punct, ",", name.line));
        }
        rest.extend(argument);
    }
    bound.push(rest);
    Ok(bound)
}

/// One step of a replacement list: a run of tokens, or a `##` between two runs.
enum Piece {
    Tokens(Vec<Token>),
    Paste,
}

fn substitute(
    body: &[Token],
    bound: Option<(&Vec<String>, &Vec<Vec<Token>>)>,
    macros: &Macros,
    hide: &[String],
    line: usize,
) -> Result<Vec<Token>, Fault> {
    let mut pieces = Vec::new();
    let mut index = 0;
    while index < body.len() {
        let token = &body[index];
        index += 1;
        if token.is("##") {
            pieces.push(Piece::Paste);
            continue;
        }
        let Some((params, arguments)) = bound else {
            pieces.push(Piece::Tokens(vec![token.clone()]));
            continue;
        };
        if token.is("#") {
            let parameter = parameter_index(params, &body[index]);
            let argument = &arguments[parameter];
            let mut string = stringify(argument, line);
            string.spaced = token.spaced;
            pieces.push(Piece::Tokens(vec![string]));
            index += 1;
            continue;
        }
        let Some(position) = (token.kind == Kind::Ident)
            .then(|| params.iter().position(|p| *p == token.text))
            .flatten()
        else {
            pieces.push(Piece::Tokens(vec![token.clone()]));
            continue;
        };
        let pasted = body.get(index).is_some_and(|next| next.is("##"))
            || (index >= 2 && body[index - 2].is("##"));
        let mut tokens = if pasted {
            arguments[position].clone()
        } else {
            expand(arguments[position].clone(), macros)?
        };
        if let Some(first) = tokens.first_mut() {
            first.spaced = token.spaced;
        }
        pieces.push(Piece::Tokens(tokens));
    }
    let mut out: Vec<Token> = Vec::new();
    let mut pasting = false;
    let mut previous_empty = false;
    for piece in pieces {
        match piece {
            Piece::Paste => pasting = true,
            Piece::Tokens(tokens) => {
                let empty = tokens.is_empty();
                let mut rest = tokens.into_iter();
                if pasting {
                    if let Some(first) = rest.next() {
                        if previous_empty || out.is_empty() {
                            out.push(first);
                        } else {
                            let left = out.pop().unwrap_or_else(|| first.clone());
                            out.extend(paste(&left, &first, line)?);
                        }
                    }
                    previous_empty = previous_empty && empty;
                } else {
                    previous_empty = empty;
                }
                out.extend(rest);
                pasting = false;
            }
        }
    }
    for token in &mut out {
        for name in hide {
            if !token.hide.contains(name) {
                token.hide.push(name.clone());
            }
        }
    }
    Ok(out)
}

fn parameter_index(params: &[String], token: &Token) -> usize {
    params
        .iter()
        .position(|p| *p == token.text)
        .unwrap_or_default()
}

fn stringify(tokens: &[Token], line: usize) -> Token {
    let mut text = String::from("\"");
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        if token.kind == Kind::Literal {
            for ch in token.text.chars() {
                if ch == '"' || ch == '\\' {
                    text.push('\\');
                }
                text.push(ch);
            }
        } else {
            text.push_str(&token.text);
        }
    }
    text.push('"');
    Token::new(Kind::Literal, text, line)
}

/// The single token `left` and `right` spell together (C17 6.10.3.3).
fn paste(left: &Token, right: &Token, line: usize) -> Result<Vec<Token>, Fault> {
    let joined = format!("{}{}", left.text, right.text);
    let mut tokens = tokenize(&joined, line)?;
    if tokens.len() != 1 {
        return Err(Fault::new(
            line,
            format!(
                "pasting `{}` and `{}` does not give a valid preprocessing token",
                left.text, right.text
            ),
        ));
    }
    for token in &mut tokens {
        token.spaced = left.spaced;
    }
    Ok(tokens)
}

/// The spelling of `tokens`: each token with a space before it where the
/// source had one, and a space between two words so they do not run together.
pub(crate) fn render(tokens: &[Token]) -> String {
    let mut text = String::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            let words = is_word(&tokens[index - 1]) && is_word(token);
            if token.spaced || words {
                text.push(' ');
            }
        }
        text.push_str(&token.text);
    }
    text
}

fn is_word(token: &Token) -> bool {
    matches!(token.kind, Kind::Ident | Kind::Number)
}
