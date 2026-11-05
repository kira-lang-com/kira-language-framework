use std::fmt;

use tir_diag::Span;
use tir_types::{Convention, Type};

use crate::text::{Token, TokenKind, lex};
use crate::{TypeParseError, parse_tvisa_type, parse_type};

pub(super) type TypeParser = fn(&str) -> Result<Type, TypeParseError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextParameter {
    pub name: String,
    pub context: bool,
    pub convention: Option<Convention>,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextInstruction {
    pub results: Vec<String>,
    pub opcode: String,
    pub tokens: Vec<Token>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasicBlock {
    pub label: String,
    pub parameters: Vec<TextParameter>,
    pub header_tokens: Vec<Token>,
    pub instructions: Vec<TextInstruction>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionBody {
    pub blocks: Vec<BasicBlock>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextFunctionSignature {
    pub parameters: Vec<TextParameter>,
    pub result: Type,
    pub failure: Option<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyParseError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for BodyParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BodyParseError {}

impl From<TypeParseError> for BodyParseError {
    fn from(error: TypeParseError) -> Self {
        Self {
            span: error.span,
            message: error.message,
        }
    }
}

#[derive(Clone)]
struct Line {
    tokens: Vec<Token>,
    span: Span,
}

/// Parses the block and instruction structure from one function or entry item.
/// Operand tokens remain available for instruction-specific verification.
pub fn parse_function_body(source: &str) -> Result<Option<FunctionBody>, BodyParseError> {
    parse_function_body_with(source, parse_type)
}

/// Parses a TVISA function body, accepting its TVISA-only value types.
pub fn parse_tvisa_function_body(source: &str) -> Result<Option<FunctionBody>, BodyParseError> {
    parse_function_body_with(source, parse_tvisa_type)
}

fn parse_function_body_with(
    source: &str,
    type_parser: TypeParser,
) -> Result<Option<FunctionBody>, BodyParseError> {
    let tokens = lex(source).map_err(|error| BodyParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    let Some((open, close)) = body_braces(&tokens)? else {
        return Ok(None);
    };
    let body_span = Span::new(tokens[open].span.end, tokens[close].span.start);
    let body_tokens = &tokens[open + 1..close];
    let lines = split_lines(body_tokens, source.len());
    let mut blocks = Vec::new();
    let mut current: Option<BasicBlock> = None;
    let entry_body = tokens.first().and_then(token_atom) == Some("entry");

    for line in lines {
        if line.tokens.is_empty() {
            continue;
        }
        if is_block_header(&line.tokens) {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some(parse_block_header(
                source,
                line,
                type_parser,
                entry_body && blocks.is_empty(),
            )?);
            continue;
        }
        let Some(block) = current.as_mut() else {
            return Err(parse_error(
                line.span,
                "instruction appears before the first block",
            ));
        };
        block.span.end = line.span.end;
        block.instructions.push(parse_instruction(line)?);
    }
    if let Some(block) = current {
        blocks.push(block);
    }
    Ok(Some(FunctionBody {
        blocks,
        span: body_span,
    }))
}

fn body_braces(tokens: &[Token]) -> Result<Option<(usize, usize)>, BodyParseError> {
    let mut depth = 0i32;
    let mut open = None;
    let mut body = None;
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = &token.kind {
            if *character == '{' {
                if depth == 0 {
                    open = Some(index);
                }
                depth += 1;
            } else if *character == '}' {
                depth -= 1;
                if depth < 0 {
                    return Err(parse_error(token.span, "unexpected closing brace"));
                }
                if depth == 0 {
                    if let Some(start) = open.take() {
                        body = Some((start, index));
                    }
                }
            }
        }
    }
    if depth != 0 {
        let span = open.map_or_else(
            || tokens.last().map_or(Span::new(0, 0), |token| token.span),
            |index| tokens[index].span,
        );
        return Err(parse_error(span, "unterminated function body"));
    }
    if let Some((_, close)) = body {
        if tokens[close + 1..]
            .iter()
            .any(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        {
            return Err(parse_error(
                tokens[close].span,
                "unexpected tokens after function body",
            ));
        }
    }
    Ok(body)
}

fn split_lines(tokens: &[Token], source_len: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line_tokens = Vec::new();
    let mut line_start = tokens.first().map_or(source_len, |token| token.span.start);
    for token in tokens {
        if matches!(token.kind, TokenKind::Newline) {
            lines.push(Line {
                tokens: std::mem::take(&mut line_tokens),
                span: Span::new(line_start, token.span.end),
            });
            line_start = token.span.end;
        } else {
            line_tokens.push(token.clone());
        }
    }
    if !line_tokens.is_empty() {
        let end = line_tokens
            .last()
            .map_or(line_start, |token| token.span.end);
        lines.push(Line {
            tokens: line_tokens,
            span: Span::new(line_start, end),
        });
    }
    lines
}

fn is_block_header(tokens: &[Token]) -> bool {
    tokens
        .last()
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(':')))
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Atom(_))
        )
}

fn parse_block_header(
    source: &str,
    line: Line,
    type_parser: TypeParser,
    entry_parameters: bool,
) -> Result<BasicBlock, BodyParseError> {
    let tokens = &line.tokens;
    let label = token_atom(&tokens[0])
        .ok_or_else(|| parse_error(tokens[0].span, "expected block label"))?
        .to_owned();
    let parameters = if tokens
        .get(1)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('(')))
    {
        let close = matching_paren(tokens, 1)?;
        if entry_parameters {
            split_entry_parameters(source, &tokens[2..close], type_parser)?
        } else {
            split_parameters(source, &tokens[2..close], type_parser)?
        }
    } else {
        Vec::new()
    };
    Ok(BasicBlock {
        label,
        parameters,
        header_tokens: tokens.clone(),
        instructions: Vec::new(),
        span: line.span,
    })
}

pub(super) fn matching_paren(tokens: &[Token], open: usize) -> Result<usize, BodyParseError> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if let TokenKind::Punctuation(character) = &token.kind {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(index);
                    }
                }
                _ => {}
            }
        }
    }
    Err(parse_error(
        tokens[open].span,
        "unterminated block parameter list",
    ))
}

pub(super) fn split_parameters(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
) -> Result<Vec<TextParameter>, BodyParseError> {
    split_parameters_with(source, tokens, type_parser, false)
}

pub(super) fn split_entry_parameters(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
) -> Result<Vec<TextParameter>, BodyParseError> {
    split_parameters_with(source, tokens, type_parser, true)
}

fn split_parameters_with(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
    entry_parameters: bool,
) -> Result<Vec<TextParameter>, BodyParseError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(token) = tokens
        .last()
        .filter(|token| matches!(token.kind, TokenKind::Punctuation(',')))
    {
        return Err(parse_error(token.span, "trailing comma in parameter list"));
    }
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = [0i32; 4];
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = &token.kind {
            match character {
                '(' => depth[0] += 1,
                ')' => depth[0] -= 1,
                '[' => depth[1] += 1,
                ']' => depth[1] -= 1,
                '<' => depth[2] += 1,
                '>' => depth[2] -= 1,
                '{' => depth[3] += 1,
                '}' => depth[3] -= 1,
                ',' if depth == [0; 4] => {
                    result.push(parse_parameter_with(
                        source,
                        &tokens[start..index],
                        type_parser,
                        entry_parameters,
                    )?);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if start < tokens.len() {
        result.push(parse_parameter_with(
            source,
            &tokens[start..],
            type_parser,
            entry_parameters,
        )?);
    }
    Ok(result)
}

pub(super) fn parse_entry_parameter(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
) -> Result<TextParameter, BodyParseError> {
    parse_parameter_with(source, tokens, type_parser, true)
}

fn parse_parameter_with(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
    entry_parameter: bool,
) -> Result<TextParameter, BodyParseError> {
    let span = Span::new(
        tokens.first().map_or(0, |token| token.span.start),
        tokens.last().map_or(0, |token| token.span.end),
    );
    if tokens.len() < 4 || !matches!(tokens[0].kind, TokenKind::Punctuation('%')) {
        return Err(parse_error(span, "malformed block parameter"));
    }
    let name = token_atom(&tokens[1])
        .ok_or_else(|| parse_error(tokens[1].span, "expected block parameter name"))?
        .to_owned();
    if !matches!(tokens[2].kind, TokenKind::Punctuation(':')) {
        return Err(parse_error(
            tokens[2].span,
            "expected ':' after block parameter",
        ));
    }
    let mut type_index = 3;
    let context = token_atom(&tokens[type_index]) == Some("context");
    if context {
        type_index += 1;
    }
    let convention = match tokens.get(type_index).and_then(token_atom) {
        Some("owned") => Some(Convention::Owned),
        Some("borrow") => Some(Convention::Borrow),
        Some("borrowmut") => Some(Convention::BorrowMut),
        _ => None,
    };
    if convention.is_some() {
        type_index += 1;
    }
    let type_token = tokens
        .get(type_index)
        .ok_or_else(|| parse_error(span, "block parameter has no type"))?;
    let end_token = tokens
        .last()
        .ok_or_else(|| parse_error(span, "block parameter has no type"))?;
    let ty = if entry_parameter {
        parse_entry_parameter_type(source, &tokens[type_index..], type_parser)?
    } else {
        parse_type_slice(source, type_token, end_token, type_parser)?
    };
    Ok(TextParameter {
        name,
        context,
        convention,
        ty,
        span,
    })
}

fn parse_entry_parameter_type(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
) -> Result<Type, BodyParseError> {
    let Some((pointer_end, pointer)) = parse_entry_pointer_prefix(source, tokens, type_parser)?
    else {
        let first = tokens
            .first()
            .ok_or_else(|| parse_error(Span::new(0, 0), "entry parameter has no type"))?;
        let last = tokens.last().unwrap_or(first);
        return parse_type_slice(source, first, last, type_parser);
    };
    let Some(pointee_start) = tokens.get(pointer_end) else {
        return Ok(pointer);
    };
    let Some(pointee_end) = tokens.last() else {
        return Err(parse_error(
            pointee_start.span,
            "entry pointer has no pointee type",
        ));
    };
    let pointee = parse_type_slice(source, pointee_start, pointee_end, type_parser)?;
    let Type::Pointer(space) = pointer else {
        return Err(parse_error(
            tokens[0].span,
            "entry pointer prefix is not a pointer",
        ));
    };
    Ok(Type::Address {
        pointee: Box::new(pointee),
        space: Some(space),
    })
}

fn parse_entry_pointer_prefix(
    source: &str,
    tokens: &[Token],
    type_parser: TypeParser,
) -> Result<Option<(usize, Type)>, BodyParseError> {
    let Some(first) = tokens.first() else {
        return Ok(None);
    };
    if token_atom(first) != Some("ptr") {
        return Ok(None);
    }
    if !tokens
        .get(1)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('<')))
    {
        return Ok(None);
    }
    let mut depth = 0usize;
    let mut close = None;
    for (index, token) in tokens.iter().enumerate().skip(1) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let Some(end) = close.map(|close| close + 1) else {
        return Ok(None);
    };
    let first = &tokens[0];
    let last = &tokens[end - 1];
    let pointer = parse_type_slice(source, first, last, type_parser)?;
    Ok(Some((end, pointer)))
}

fn parse_type_slice(
    source: &str,
    first: &Token,
    last: &Token,
    type_parser: TypeParser,
) -> Result<Type, BodyParseError> {
    type_parser(&source[first.span.start..last.span.end]).map_err(|error| BodyParseError {
        span: Span::new(
            first.span.start + error.span.start,
            first.span.start + error.span.end,
        ),
        message: error.message,
    })
}

fn parse_instruction(line: Line) -> Result<TextInstruction, BodyParseError> {
    let tokens = line.tokens;
    let mut cursor = 0;
    let mut results = Vec::new();
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        loop {
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            ) {
                return Err(parse_error(
                    tokens.get(cursor).map_or(line.span, |token| token.span),
                    "expected result value",
                ));
            }
            let name_token = tokens
                .get(cursor + 1)
                .ok_or_else(|| parse_error(line.span, "expected result name"))?;
            let name = token_atom(name_token)
                .ok_or_else(|| parse_error(name_token.span, "expected result name"))?
                .to_owned();
            results.push(name);
            cursor += 2;
            if matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('='))
            ) {
                cursor += 1;
                break;
            }
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation(','))
            ) {
                return Err(parse_error(
                    tokens.get(cursor).map_or(line.span, |token| token.span),
                    "expected ',' or '=' after result value",
                ));
            }
            cursor += 1;
        }
    }
    let opcode_start = cursor;
    let mut opcode = token_atom(
        tokens
            .get(cursor)
            .ok_or_else(|| parse_error(line.span, "instruction has no opcode"))?,
    )
    .ok_or_else(|| parse_error(line.span, "instruction has no opcode"))?
    .to_owned();
    cursor += 1;
    while cursor + 1 < tokens.len() && matches!(tokens[cursor].kind, TokenKind::Punctuation('.')) {
        let component = token_atom(&tokens[cursor + 1])
            .ok_or_else(|| parse_error(tokens[cursor + 1].span, "malformed opcode"))?;
        opcode.push('.');
        opcode.push_str(component);
        cursor += 2;
    }
    if cursor == opcode_start {
        return Err(parse_error(line.span, "instruction has no opcode"));
    }
    Ok(TextInstruction {
        results,
        opcode,
        tokens,
        span: line.span,
    })
}

pub(super) fn token_atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(atom) = &token.kind {
        Some(atom)
    } else {
        None
    }
}

pub(super) fn parse_error(span: Span, message: &str) -> BodyParseError {
    BodyParseError {
        span,
        message: message.to_owned(),
    }
}
