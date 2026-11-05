use crate::text::{TokenKind, decode_kira_byte_string, decode_kira_string_literal_contents};
use crate::{BasicBlock, TextInstruction, TextItem, TextParseError};
use tir_diag::Span;
use tir_types::{AddressSpace, FloatType, Type};

pub(crate) enum ParsedEntryConstant {
    Integer(ParsedConstInt),
    Boolean(ParsedConstBool),
    Float(ParsedConstFloat),
    Null(ParsedConstNull),
    Zero(ParsedConstZero),
    Bytes(ParsedConstBytes),
    String(ParsedConstString),
    Poison(ParsedConstZero),
}

impl ParsedEntryConstant {
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Integer(constant) => &constant.name,
            Self::Boolean(constant) => &constant.name,
            Self::Float(constant) => &constant.name,
            Self::Null(constant) => &constant.name,
            Self::Zero(constant) => &constant.name,
            Self::Bytes(constant) => &constant.name,
            Self::String(constant) => &constant.name,
            Self::Poison(constant) => &constant.name,
        }
    }

    pub(crate) fn ty(&self) -> &Type {
        match self {
            Self::Integer(constant) => &constant.ty,
            Self::Boolean(constant) => &constant.ty,
            Self::Float(constant) => &constant.ty,
            Self::Null(constant) => &constant.ty,
            Self::Zero(constant) => &constant.ty,
            Self::Bytes(constant) => &constant.ty,
            Self::String(constant) => &constant.ty,
            Self::Poison(constant) => &constant.ty,
        }
    }
}

pub(crate) struct ParsedConstInt {
    name: String,
    pub(crate) ty: Type,
    pub(crate) value: u128,
}

pub(crate) struct ParsedConstZero {
    name: String,
    ty: Type,
}

pub(crate) struct ParsedConstBool {
    name: String,
    ty: Type,
    pub(crate) value: bool,
}

pub(crate) struct ParsedConstFloat {
    name: String,
    pub(crate) ty: Type,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct ParsedConstNull {
    name: String,
    ty: Type,
    pub(crate) space_code: u8,
}

pub(crate) struct ParsedConstBytes {
    name: String,
    pub(crate) ty: Type,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct ParsedConstString {
    name: String,
    ty: Type,
    pub(crate) value: String,
}

struct NamedConstant<T> {
    name: String,
    value: T,
}

pub(crate) fn supported_instruction_shape(block: &BasicBlock) -> bool {
    match block.instructions.as_slice() {
        [instruction] => instruction.opcode == "return" && instruction.results.is_empty(),
        [constant, result] => {
            matches!(
                constant.opcode.as_str(),
                "const.int"
                    | "const.bool"
                    | "const.float"
                    | "const.null"
                    | "const.zero"
                    | "const.bytes"
                    | "const.string"
                    | "const.poison"
                    | "add"
                    | "sub"
                    | "mul"
                    | "and"
                    | "or"
                    | "xor"
                    | "not"
                    | "neg"
                    | "abs"
                    | "icmp"
                    | "select"
            ) && constant.results.len() == 1
                && result.opcode == "return"
                && result.results.is_empty()
        }
        _ => false,
    }
}

pub(crate) fn parse_entry_constant(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<ParsedEntryConstant, TextParseError> {
    match instruction.opcode.as_str() {
        "const.int" => parse_const_int(instruction, source, item).map(|constant| {
            let (ty, value) = constant.value;
            ParsedEntryConstant::Integer(ParsedConstInt {
                name: constant.name,
                ty,
                value,
            })
        }),
        "const.bool" => parse_const_bool(instruction, item).map(|constant| {
            ParsedEntryConstant::Boolean(ParsedConstBool {
                name: constant.name,
                ty: Type::Integer(1),
                value: constant.value,
            })
        }),
        "const.float" => parse_const_float(instruction, source, item).map(|constant| {
            ParsedEntryConstant::Float(ParsedConstFloat {
                name: constant.name,
                ty: constant.value.0,
                bytes: constant.value.1,
            })
        }),
        "const.null" => parse_const_null(instruction, item),
        "const.zero" => parse_const_zero(instruction, source, item).map(|constant| {
            ParsedEntryConstant::Zero(ParsedConstZero {
                name: constant.name,
                ty: constant.value,
            })
        }),
        "const.poison" => parse_const_typed(instruction, source, item, "poison").map(|constant| {
            ParsedEntryConstant::Poison(ParsedConstZero {
                name: constant.name,
                ty: constant.value,
            })
        }),
        "const.bytes" => {
            parse_const_bytes(instruction, source, item).map(ParsedEntryConstant::Bytes)
        }
        "const.string" => parse_const_string(instruction, item).map(ParsedEntryConstant::String),
        _ => Err(item_error(
            item,
            instruction.span,
            "unsupported constant instruction",
        )),
    }
}

fn parse_const_string(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<ParsedConstString, TextParseError> {
    let tokens = &instruction.tokens;
    if instruction.opcode != "const.string"
        || instruction.results.len() != 1
        || tokens.len() != 7
        || !matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        || !matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        || !matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        || !matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        || !matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "string")
    {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.string \"text\"`",
        ));
    }
    let Some(TokenKind::String(raw)) = tokens.get(6).map(|token| &token.kind) else {
        return Err(item_error(
            item,
            instruction.span,
            "const.string requires a string literal",
        ));
    };
    let value = decode_kira_string_literal_contents(raw).map_err(|error| {
        item_error(
            item,
            tokens[6].span,
            format!("invalid const.string literal: {error}"),
        )
    })?;
    Ok(ParsedConstString {
        name: result_name(instruction, item)?,
        ty: Type::Tuple(vec![
            Type::Pointer(AddressSpace::Generic),
            Type::Integer(64),
        ]),
        value,
    })
}

fn parse_const_bytes(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<ParsedConstBytes, TextParseError> {
    let tokens = &instruction.tokens;
    let valid_prefix = instruction.opcode == "const.bytes"
        && instruction.results.len() == 1
        && tokens.len() >= 10
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "bytes");
    let comma = tokens.len().saturating_sub(3);
    if !valid_prefix
        || !matches!(
            tokens.get(comma).map(|token| &token.kind),
            Some(TokenKind::Punctuation(','))
        )
        || !matches!(tokens.get(comma + 1).map(|token| &token.kind), Some(TokenKind::Atom(prefix)) if prefix == "b")
    {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.bytes T, b\"...\"`",
        ));
    }
    let Some(TokenKind::String(raw)) = tokens.get(comma + 2).map(|token| &token.kind) else {
        return Err(item_error(
            item,
            instruction.span,
            "const.bytes requires a byte string literal",
        ));
    };
    let type_first = tokens
        .get(6)
        .ok_or_else(|| item_error(item, instruction.span, "const.bytes is missing its type"))?;
    let type_last = tokens
        .get(comma - 1)
        .ok_or_else(|| item_error(item, instruction.span, "const.bytes is missing its type"))?;
    let type_source = source
        .get(type_first.span.start..type_last.span.end)
        .ok_or_else(|| {
            item_error(
                item,
                type_first.span,
                "const.bytes type has an invalid span",
            )
        })?;
    let ty = crate::parse_type(type_source).map_err(|error| {
        item_error(
            item,
            Span::new(
                type_first.span.start.saturating_add(error.span.start),
                type_first.span.start.saturating_add(error.span.end),
            ),
            error.message,
        )
    })?;
    let bytes = decode_kira_byte_string(raw).map_err(|error| {
        item_error(
            item,
            tokens[comma + 2].span,
            format!("invalid const.bytes literal: {error}"),
        )
    })?;
    Ok(ParsedConstBytes {
        name: result_name(instruction, item)?,
        ty,
        bytes,
    })
}

fn parse_const_null(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<ParsedEntryConstant, TextParseError> {
    let tokens = &instruction.tokens;
    let valid = instruction.opcode == "const.null"
        && instruction.results.len() == 1
        && (tokens.len() == 6 || tokens.len() == 7)
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "null");
    if !valid {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.null [address-space]`",
        ));
    }
    let name = result_name(instruction, item)?;
    let (space, space_code) = match tokens.get(6).map(|token| &token.kind) {
        None => (AddressSpace::Generic, 0),
        Some(TokenKind::Atom(space)) => match space.as_str() {
            "generic" => (AddressSpace::Generic, 0),
            "device" => (AddressSpace::Device, 1),
            "constant" => (AddressSpace::Constant, 2),
            "threadgroup" => (AddressSpace::Threadgroup, 3),
            "thread" => (AddressSpace::Thread, 4),
            "object_payload" => (AddressSpace::ObjectPayload, 5),
            "ray_payload" => (AddressSpace::RayPayload, 6),
            "hit_attribute" => (AddressSpace::HitAttribute, 7),
            "tile" => (AddressSpace::Tile, 8),
            "node_record" => (AddressSpace::NodeRecord, 9),
            _ => {
                return Err(item_error(
                    item,
                    tokens[6].span,
                    "unknown const.null address space",
                ));
            }
        },
        Some(_) => {
            return Err(item_error(
                item,
                tokens[6].span,
                "invalid const.null address space",
            ));
        }
    };
    Ok(ParsedEntryConstant::Null(ParsedConstNull {
        name,
        ty: Type::Pointer(space),
        space_code,
    }))
}

fn parse_const_bool(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<NamedConstant<bool>, TextParseError> {
    let tokens = &instruction.tokens;
    let valid = instruction.opcode == "const.bool"
        && instruction.results.len() == 1
        && tokens.len() == 7
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "bool");
    if !valid {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.bool true|false`",
        ));
    }
    let name = result_name(instruction, item)?;
    let value = match &tokens[6].kind {
        TokenKind::Atom(value) if value == "true" => true,
        TokenKind::Atom(value) if value == "false" => false,
        _ => {
            return Err(item_error(
                item,
                tokens[6].span,
                "const.bool expects true or false",
            ));
        }
    };
    Ok(NamedConstant { name, value })
}

fn parse_const_zero(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<NamedConstant<Type>, TextParseError> {
    parse_const_typed(instruction, source, item, "zero")
}

fn parse_const_typed(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    kind: &str,
) -> Result<NamedConstant<Type>, TextParseError> {
    let tokens = &instruction.tokens;
    let expected_opcode = format!("const.{kind}");
    let valid = instruction.opcode == expected_opcode
        && instruction.results.len() == 1
        && tokens.len() >= 7
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == kind);
    if !valid {
        return Err(item_error(
            item,
            instruction.span,
            format!("expected `%name = const.{kind} T`"),
        ));
    }
    let name = result_name(instruction, item)?;
    let type_token = &tokens[6];
    let type_end = tokens
        .last()
        .map_or(type_token.span.end, |token| token.span.end);
    let type_source = source.get(type_token.span.start..type_end).ok_or_else(|| {
        item_error(
            item,
            type_token.span,
            format!("const.{kind} type has an invalid span"),
        )
    })?;
    let ty = crate::parse_type(type_source).map_err(|error| {
        item_error(
            item,
            Span::new(
                type_token.span.start.saturating_add(error.span.start),
                type_token.span.start.saturating_add(error.span.end),
            ),
            error.message,
        )
    })?;
    Ok(NamedConstant { name, value: ty })
}

fn parse_const_int(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<NamedConstant<(Type, u128)>, TextParseError> {
    let tokens = &instruction.tokens;
    let shape_matches = instruction.opcode == "const.int"
        && instruction.results.len() == 1
        && tokens.len() >= 9
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "int")
        && matches!(
            tokens.get(7).map(|token| &token.kind),
            Some(TokenKind::Punctuation(','))
        );
    if !shape_matches {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.int iN, integer-literal`",
        ));
    }
    let name = result_name(instruction, item)?;
    let type_token = &tokens[6];
    let type_source = source
        .get(type_token.span.start..type_token.span.end)
        .ok_or_else(|| item_error(item, type_token.span, "const.int type has an invalid span"))?;
    let ty = crate::parse_type(type_source)
        .map_err(|error| item_error(item, error.span, error.message))?;
    let Type::Integer(width) = ty else {
        return Err(item_error(
            item,
            type_token.span,
            "const.int type must be an integer",
        ));
    };
    let (literal_start, literal_end, has_sign) = if matches!(
        tokens.get(8).map(|token| &token.kind),
        Some(TokenKind::Punctuation('+' | '-'))
    ) {
        let literal = tokens
            .get(9)
            .ok_or_else(|| item_error(item, instruction.span, "const.int literal is missing"))?;
        if !matches!(literal.kind, TokenKind::Atom(_)) {
            return Err(item_error(item, literal.span, "invalid const.int literal"));
        }
        (tokens[8].span.start, literal.span.end, true)
    } else {
        let literal = tokens
            .get(8)
            .ok_or_else(|| item_error(item, instruction.span, "const.int literal is missing"))?;
        if !matches!(literal.kind, TokenKind::Atom(_)) {
            return Err(item_error(item, literal.span, "invalid const.int literal"));
        }
        (literal.span.start, literal.span.end, false)
    };
    if tokens.len() != if has_sign { 10 } else { 9 } {
        return Err(item_error(
            item,
            instruction.span,
            "unexpected const.int operands",
        ));
    }
    let literal = source.get(literal_start..literal_end).ok_or_else(|| {
        item_error(
            item,
            instruction.span,
            "const.int literal has an invalid span",
        )
    })?;
    let value = crate::numeric_literals::integer_literal_bits(literal, width)
        .ok_or_else(|| item_error(item, instruction.span, "const.int literal is out of range"))?;
    Ok(NamedConstant {
        name,
        value: (Type::Integer(width), value),
    })
}

fn parse_const_float(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<NamedConstant<(Type, Vec<u8>)>, TextParseError> {
    let tokens = &instruction.tokens;
    let valid = instruction.opcode == "const.float"
        && instruction.results.len() == 1
        && tokens.len() >= 9
        && matches!(
            tokens.first().map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
        && matches!(
            tokens.get(2).map(|token| &token.kind),
            Some(TokenKind::Punctuation('='))
        )
        && matches!(tokens.get(3).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "const")
        && matches!(
            tokens.get(4).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        )
        && matches!(tokens.get(5).map(|token| &token.kind), Some(TokenKind::Atom(opcode)) if opcode == "float")
        && matches!(
            tokens.get(7).map(|token| &token.kind),
            Some(TokenKind::Punctuation(','))
        );
    if !valid {
        return Err(item_error(
            item,
            instruction.span,
            "expected `%name = const.float fN, float-literal`",
        ));
    }
    let name = result_name(instruction, item)?;
    let type_token = &tokens[6];
    let type_source = source
        .get(type_token.span.start..type_token.span.end)
        .ok_or_else(|| {
            item_error(
                item,
                type_token.span,
                "const.float type has an invalid span",
            )
        })?;
    let ty = crate::parse_type(type_source)
        .map_err(|error| item_error(item, error.span, error.message))?;
    let Type::Float(
        float @ (FloatType::F16
        | FloatType::Bf16
        | FloatType::F32
        | FloatType::F64
        | FloatType::F128),
    ) = ty
    else {
        return Err(item_error(
            item,
            type_token.span,
            "const.float type must be f16, bf16, f32, f64, or f128",
        ));
    };
    let start = tokens[8].span.start;
    let end = tokens
        .last()
        .map_or(tokens[8].span.end, |token| token.span.end);
    let literal = source.get(start..end).ok_or_else(|| {
        item_error(
            item,
            instruction.span,
            "const.float literal has an invalid span",
        )
    })?;
    if !crate::numeric_literals::float_literal_is_exactly_representable(literal, float) {
        return Err(item_error(
            item,
            instruction.span,
            "const.float literal is not exactly representable",
        ));
    }
    let bytes = crate::numeric_literals::parse_ieee_float_literal_bytes(literal, float)
        .ok_or_else(|| item_error(item, instruction.span, "invalid const.float literal"))?;
    Ok(NamedConstant {
        name,
        value: (Type::Float(float), bytes),
    })
}

fn result_name(instruction: &TextInstruction, item: TextItem) -> Result<String, TextParseError> {
    match instruction.tokens.get(1).map(|token| &token.kind) {
        Some(TokenKind::Atom(name)) if name == &instruction.results[0] => Ok(name.clone()),
        _ => Err(item_error(
            item,
            instruction.span,
            "invalid constant result name",
        )),
    }
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
