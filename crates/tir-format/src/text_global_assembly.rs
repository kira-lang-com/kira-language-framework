use crate::text::{
    Token, TokenKind, decode_kira_byte_string, decode_kira_string_literal_contents, lex,
};
use crate::{
    BinaryAddressSpace, BinaryConstantValue, BinaryGlobal, BinaryLinkage, TextItem, TextParseError,
    parse_type,
};
use tir_diag::Span;
use tir_types::{AddressSpace, FloatType, Type};

pub(crate) struct ParsedGlobal {
    pub(crate) record: BinaryGlobal,
    pub(crate) ty: Type,
    pub(crate) initializer: Option<BinaryConstantValue>,
    pub(crate) item: TextItem,
}

pub(crate) fn parse_global(source: &str, item: TextItem) -> Result<ParsedGlobal, TextParseError> {
    let tokens = lex(source).map_err(|error| item_error(item, error.span, error.message))?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect::<Vec<_>>();
    let mut cursor = Cursor {
        tokens: &tokens,
        index: 0,
        item,
    };
    cursor.expect_atom("global")?;
    let linkage = match cursor.atom_value()?.as_str() {
        "private" => BinaryLinkage::Private,
        "module" => BinaryLinkage::Module,
        "package" => BinaryLinkage::Package,
        "exported" => BinaryLinkage::Exported,
        _ => return Err(cursor.error("unknown global linkage")),
    };
    cursor.expect_punctuation('@')?;
    let name = cursor.path()?;
    cursor.expect_punctuation(':')?;

    let type_start = cursor.index;
    let (type_end, address_space, attributes_start) =
        find_suffix(source, item, &tokens, type_start)?;
    let ty = parse_type_range(source, item, &tokens, type_start, type_end)?;
    cursor.index = attributes_start;
    let address_space = address_space.unwrap_or(BinaryAddressSpace::Generic);
    let initializer = if cursor.eat_punctuation('=') {
        Some(parse_initializer(source, &mut cursor, &ty)?)
    } else {
        None
    };
    let mut thread_local = false;
    let mut constant = false;
    let mut per_cpu = false;
    let mut alignment = None;
    while cursor.index < tokens.len() {
        let attribute = cursor.atom_value()?;
        match attribute.as_str() {
            "thread_local" if !thread_local => thread_local = true,
            "constant" if !constant => constant = true,
            "percpu" if !per_cpu => per_cpu = true,
            "align" if alignment.is_none() => {
                cursor.expect_punctuation('(')?;
                let token = cursor.next()?;
                let TokenKind::Atom(value) = token.kind else {
                    return Err(cursor.error_at(token.span, "expected global alignment"));
                };
                let alignment_value = crate::numeric_literals::parse_unsigned_integer(&value)
                    .and_then(|value| u64::try_from(value).ok())
                    .ok_or_else(|| cursor.error_at(token.span, "invalid global alignment"))?;
                if !alignment_value.is_power_of_two() {
                    return Err(
                        cursor.error_at(token.span, "global alignment must be a power of two")
                    );
                }
                cursor.expect_punctuation(')')?;
                alignment = Some(alignment_value);
            }
            "thread_local" | "constant" | "percpu" | "align" => {
                return Err(cursor.error("duplicate global attribute"));
            }
            _ => return Err(cursor.error("unknown global attribute or unsupported initializer")),
        }
    }

    Ok(ParsedGlobal {
        record: BinaryGlobal {
            name,
            linkage,
            ty: 0,
            address_space,
            thread_local,
            constant,
            per_cpu,
            alignment,
            initializer: None,
        },
        ty,
        initializer,
        item,
    })
}

fn parse_initializer(
    source: &str,
    cursor: &mut Cursor<'_>,
    ty: &Type,
) -> Result<BinaryConstantValue, TextParseError> {
    let first = cursor.next()?;
    if let TokenKind::String(raw) = &first.kind {
        if !is_string_type(ty) {
            return Err(
                cursor.error_at(first.span, "string initializer requires type `(ptr, i64)`")
            );
        }
        let value = decode_kira_string_literal_contents(raw).map_err(|error| {
            cursor.error_at(first.span, format!("invalid string escape: {error}"))
        })?;
        return Ok(BinaryConstantValue::String(value));
    }
    if matches!(&first.kind, TokenKind::Atom(value) if value == "b") {
        let token = cursor.next()?;
        let TokenKind::String(raw) = token.kind else {
            return Err(cursor.error_at(token.span, "expected byte string literal"));
        };
        return decode_kira_byte_string(&raw)
            .map(BinaryConstantValue::Bytes)
            .map_err(|error| cursor.error_at(token.span, format!("invalid byte string: {error}")));
    }
    let literal = take_literal(source, cursor, &first)?;

    if let Type::Integer(1) = ty {
        if literal == "true" {
            return Ok(BinaryConstantValue::Integer(vec![1]));
        }
        if literal == "false" {
            return Ok(BinaryConstantValue::Zero);
        }
    }
    if literal == "0" {
        return Ok(BinaryConstantValue::Zero);
    }
    if matches!(ty, Type::Pointer(_)) && literal == "null" {
        return Ok(BinaryConstantValue::Null);
    }
    match ty {
        Type::Integer(bits) => {
            let value = crate::numeric_literals::integer_literal_bits(&literal, *bits).ok_or_else(
                || cursor.error_at(first.span, "integer initializer is out of range"),
            )?;
            let byte_count = usize::from(bits.div_ceil(8));
            Ok(BinaryConstantValue::Integer(
                value.to_le_bytes()[..byte_count].to_vec(),
            ))
        }
        Type::Float(
            float @ (FloatType::F16
            | FloatType::Bf16
            | FloatType::F32
            | FloatType::F64
            | FloatType::F128),
        ) => {
            if !crate::numeric_literals::float_literal_is_exactly_representable(&literal, *float) {
                return Err(cursor.error_at(
                    first.span,
                    "floating initializer is not exactly representable",
                ));
            }
            let bytes =
                crate::numeric_literals::parse_ieee_float_literal_bytes(&literal, *float)
                    .ok_or_else(|| cursor.error_at(first.span, "invalid floating initializer"))?;
            Ok(BinaryConstantValue::Float(bytes))
        }
        _ => Err(cursor.error_at(
            first.span,
            "textual global initializers currently support integers, f32, f64, booleans, and null",
        )),
    }
}

fn take_literal(
    source: &str,
    cursor: &mut Cursor<'_>,
    first: &Token,
) -> Result<String, TextParseError> {
    let start = first.span.start;
    let rest = source
        .get(start..)
        .ok_or_else(|| cursor.error_at(first.span, "initializer has an invalid source span"))?;
    let length = rest
        .char_indices()
        .find_map(|(index, character)| {
            (character.is_whitespace() || character == ';').then_some(index)
        })
        .unwrap_or(rest.len());
    if length == 0 {
        return Err(cursor.error_at(first.span, "initializer literal is empty"));
    }
    let end = start.saturating_add(length);
    let literal = source
        .get(start..end)
        .ok_or_else(|| cursor.error_at(first.span, "initializer has an invalid source span"))?
        .to_owned();
    while cursor
        .tokens
        .get(cursor.index)
        .is_some_and(|token| token.span.end <= end)
    {
        cursor.index += 1;
    }
    Ok(literal)
}

fn is_string_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Tuple(elements)
            if elements.as_slice()
                == [
                    Type::Pointer(AddressSpace::Generic),
                    Type::Integer(64),
                ]
    )
}

fn find_suffix(
    source: &str,
    item: TextItem,
    tokens: &[Token],
    start: usize,
) -> Result<(usize, Option<BinaryAddressSpace>, usize), TextParseError> {
    let mut nesting = Vec::new();
    for index in start..tokens.len() {
        if nesting.is_empty() {
            if matches!(tokens[index].kind, TokenKind::Punctuation('='))
                || is_attribute(&tokens[index])
            {
                if parse_type_range(source, item, tokens, start, index).is_ok() {
                    return Ok((index, None, index));
                }
            }
            if punctuation(&tokens[index], '<')
                && let Some((space, after)) = address_space_clause(tokens, index)
                && parse_type_range(source, item, tokens, start, index).is_ok()
            {
                return Ok((index, Some(space), after));
            }
        }
        match tokens[index].kind {
            TokenKind::Punctuation(open @ ('(' | '[' | '{' | '<')) => nesting.push(open),
            TokenKind::Punctuation(close @ (')' | ']' | '}' | '>')) => {
                if nesting.last().is_some_and(|open| {
                    matches!(
                        (*open, close),
                        ('(', ')') | ('[', ']') | ('{', '}') | ('<', '>')
                    )
                }) {
                    nesting.pop();
                }
            }
            _ => {}
        }
    }
    Ok((tokens.len(), None, tokens.len()))
}

fn address_space_clause(tokens: &[Token], start: usize) -> Option<(BinaryAddressSpace, usize)> {
    if !punctuation(tokens.get(start)?, '<') || !punctuation(tokens.get(start + 2)?, '>') {
        return None;
    }
    let TokenKind::Atom(name) = &tokens.get(start + 1)?.kind else {
        return None;
    };
    Some((address_space(name)?, start + 3))
}

fn address_space(name: &str) -> Option<BinaryAddressSpace> {
    Some(match name {
        "generic" => BinaryAddressSpace::Generic,
        "device" => BinaryAddressSpace::Device,
        "constant" => BinaryAddressSpace::Constant,
        "threadgroup" => BinaryAddressSpace::Threadgroup,
        "thread" => BinaryAddressSpace::Thread,
        "object_payload" => BinaryAddressSpace::ObjectPayload,
        "ray_payload" => BinaryAddressSpace::RayPayload,
        "hit_attribute" => BinaryAddressSpace::HitAttribute,
        "tile" => BinaryAddressSpace::Tile,
        "node_record" => BinaryAddressSpace::NodeRecord,
        _ => return None,
    })
}

fn is_attribute(token: &Token) -> bool {
    matches!(
        &token.kind,
        TokenKind::Atom(name)
            if matches!(name.as_str(), "thread_local" | "constant" | "percpu" | "align")
    )
}

fn parse_type_range(
    source: &str,
    item: TextItem,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Result<Type, TextParseError> {
    let first = tokens
        .get(start)
        .ok_or_else(|| item_error(item, Span::new(0, 0), "global is missing its type"))?;
    let last = tokens
        .get(
            end.checked_sub(1)
                .ok_or_else(|| item_error(item, first.span, "global is missing its type"))?,
        )
        .ok_or_else(|| item_error(item, first.span, "global is missing its type"))?;
    let text = source
        .get(first.span.start..last.span.end)
        .ok_or_else(|| item_error(item, first.span, "global type has an invalid source span"))?;
    parse_type(text).map_err(|error| {
        item_error(
            item,
            Span::new(
                first.span.start.saturating_add(error.span.start),
                first.span.start.saturating_add(error.span.end),
            ),
            error.message,
        )
    })
}

struct Cursor<'a> {
    tokens: &'a [Token],
    index: usize,
    item: TextItem,
}

impl Cursor<'_> {
    fn atom_value(&mut self) -> Result<String, TextParseError> {
        let token = self.next()?;
        if let TokenKind::Atom(value) = token.kind {
            Ok(value)
        } else {
            Err(self.error_at(token.span, "expected identifier"))
        }
    }

    fn path(&mut self) -> Result<String, TextParseError> {
        let mut path = self.atom_value()?;
        while self.eat_punctuation('.') {
            path.push('.');
            path.push_str(&self.atom_value()?);
        }
        Ok(path)
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        if self.atom_value()? == expected {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{expected}`")))
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{expected}`")))
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if self.tokens.get(self.index).is_some_and(
            |token| matches!(token.kind, TokenKind::Punctuation(value) if value == expected),
        ) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Result<Token, TextParseError> {
        let Some(token) = self.tokens.get(self.index).cloned() else {
            return Err(self.error("unexpected end of global declaration"));
        };
        self.index += 1;
        Ok(token)
    }

    fn error(&self, message: &str) -> TextParseError {
        let span = self
            .tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .map_or(self.item.span, |token| token.span);
        self.error_at(span, message)
    }

    fn error_at(&self, span: Span, message: impl Into<String>) -> TextParseError {
        item_error(self.item, span, message)
    }
}

fn punctuation(token: &Token, value: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == value)
}

fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
