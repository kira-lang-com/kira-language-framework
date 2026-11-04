use tir_diag::Span;
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TypeParseError, parse_tvisa_type};
use tir_types::{FloatType, Type};

use crate::text::{TextParseError, error};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextCallingConvention {
    Ridl,
    C,
    Rtcc,
    Gpu,
    Entry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextSignature {
    pub name: String,
    pub convention: TextCallingConvention,
    pub fails: bool,
    pub variadic: Option<u32>,
    pub noreturn: bool,
    pub parameters: Vec<ParameterItem>,
    pub results: Vec<ResultItem>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterItem {
    Leaf(Type),
    Group(SignatureGroup),
    ByRef {
        size: u64,
        align: u64,
        flags: Vec<String>,
    },
    Sret {
        size: u64,
        align: u64,
    },
    Context,
    CAggregate(CAggregateDescriptor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultItem {
    Leaf(Type),
    Group(SignatureGroup),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureGroup {
    pub size: u64,
    pub align: u64,
    pub members: Vec<SignatureMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureMember {
    pub offset: u64,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CAggregateDescriptor {
    pub size: u64,
    pub align: u64,
    pub members: Vec<CAggregateMember>,
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CAggregateMember {
    pub offset: u64,
    pub ty: Type,
    pub bit_width: Option<u32>,
}

pub(crate) fn parse_signature(source: &str) -> Result<TextSignature, TextParseError> {
    let tokens = lex(source).map_err(|cause| TextParseError {
        span: cause.span,
        message: cause.message.to_owned(),
    })?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(error(Span::new(0, 0), "expected TVISA signature"));
    }
    let mut parser = Parser {
        source,
        tokens: &tokens,
        index: 0,
    };
    parser.expect_atom("sig")?;
    parser.expect_punctuation('$')?;
    let name = parser.atom("expected signature identifier")?;
    parser.expect_punctuation('=')?;
    let convention_span = parser.peek().span;
    let convention = match parser.atom("expected signature convention")?.as_str() {
        "ridl" => TextCallingConvention::Ridl,
        "c" => TextCallingConvention::C,
        "rtcc" => TextCallingConvention::Rtcc,
        "gpu" => TextCallingConvention::Gpu,
        "entry" => TextCallingConvention::Entry,
        _ => return Err(error(convention_span, "unknown TVISA calling convention")),
    };
    let fails = parser.eat_atom("fails");
    let variadic = if parser.eat_atom("variadic") {
        Some(parser.number("invalid variadic parameter count")?)
    } else {
        None
    };
    let noreturn = parser.eat_atom("noreturn");
    parser.expect_punctuation('(')?;
    let parameters = parser.list(')', |parser| parser.parameter_item())?;
    parser.expect_arrow()?;
    parser.expect_punctuation('(')?;
    let results = parser.list(')', |parser| parser.result_item())?;
    if parser.index != parser.tokens.len() {
        return Err(error(
            parser.peek().span,
            "unexpected token after signature",
        ));
    }
    let span = Span::new(
        tokens.first().map_or(0, |token| token.span.start),
        tokens.last().map_or(0, |token| token.span.end),
    );
    let signature = TextSignature {
        name,
        convention,
        fails,
        variadic,
        noreturn,
        parameters,
        results,
        span,
    };
    validate_signature(&signature, span)?;
    Ok(signature)
}

fn validate_signature(signature: &TextSignature, span: Span) -> Result<(), TextParseError> {
    if signature.parameters.len() >= 1 << 16 {
        return Err(error(span, "signature item count exceeds the TVISA limit"));
    }
    if signature
        .variadic
        .is_some_and(|count| count as usize > signature.parameters.len())
    {
        return Err(error(
            span,
            "variadic fixed-parameter count exceeds the signature",
        ));
    }
    if signature.variadic.is_some() && signature.convention != TextCallingConvention::C {
        return Err(error(
            span,
            "TVISA variadic signatures must use the c convention",
        ));
    }
    let contexts = signature
        .parameters
        .iter()
        .filter(|item| matches!(item, ParameterItem::Context))
        .count();
    let sret = signature
        .parameters
        .iter()
        .filter(|item| matches!(item, ParameterItem::Sret { .. }))
        .count();
    if contexts > 1 || sret > 1 {
        return Err(error(
            span,
            "signature permits at most one context and one sret item",
        ));
    }
    if contexts != 0 && signature.convention != TextCallingConvention::Ridl {
        return Err(error(span, "context items require the ridl convention"));
    }
    if sret != 0 && !signature.results.is_empty() {
        return Err(error(span, "sret signatures cannot have result items"));
    }
    if signature.convention != TextCallingConvention::C
        && signature
            .parameters
            .iter()
            .any(|item| matches!(item, ParameterItem::CAggregate(_)))
    {
        return Err(error(span, "cagg items require the c convention"));
    }
    if signature.convention == TextCallingConvention::C
        && signature
            .parameters
            .iter()
            .any(|item| matches!(item, ParameterItem::Group(_) | ParameterItem::Context))
    {
        return Err(error(
            span,
            "c signatures permit only leaf, byref, sret, and cagg parameter items",
        ));
    }
    for item in &signature.parameters {
        match item {
            ParameterItem::ByRef { flags, .. } => {
                validate_flag_names(flags, &["owned", "readonly"], "byref", span)?;
            }
            ParameterItem::CAggregate(aggregate) => {
                validate_flag_names(
                    &aggregate.flags,
                    &["union", "unaligned", "nontrivial", "flexible", "empty"],
                    "C aggregate",
                    span,
                )?;
            }
            _ => {}
        }
    }
    if signature
        .results
        .iter()
        .filter(|item| matches!(item, ResultItem::Group(_)))
        .count()
        > 1
    {
        return Err(error(span, "a signature may have at most one result group"));
    }
    Ok(())
}

fn validate_flag_names(
    flags: &[String],
    allowed: &[&str],
    label: &str,
    span: Span,
) -> Result<(), TextParseError> {
    for (index, flag) in flags.iter().enumerate() {
        if !allowed.contains(&flag.as_str()) {
            return Err(TextParseError {
                span,
                message: format!("unknown {label} flag `{flag}`"),
            });
        }
        if flags[..index].contains(flag) {
            return Err(TextParseError {
                span,
                message: format!("duplicate {label} flag `{flag}`"),
            });
        }
    }
    Ok(())
}

struct Parser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    index: usize,
}

impl Parser<'_> {
    fn parameter_item(&mut self) -> Result<ParameterItem, TextParseError> {
        let kind = self.atom("expected parameter item")?;
        match kind.as_str() {
            "leaf" => self.parse_type(&[',', ')']).map(ParameterItem::Leaf),
            "group" => self.group().map(ParameterItem::Group),
            "byref" => {
                let size = self.number64("invalid byref size")?;
                let align = self.number64("invalid byref alignment")?;
                let flags = self.flags()?;
                validate_alignment(align, self.tokens[self.index - 1].span)?;
                Ok(ParameterItem::ByRef { size, align, flags })
            }
            "sret" => {
                let size = self.number64("invalid sret size")?;
                let align = self.number64("invalid sret alignment")?;
                validate_alignment(align, self.tokens[self.index - 1].span)?;
                Ok(ParameterItem::Sret { size, align })
            }
            "context" => Ok(ParameterItem::Context),
            "cagg" => self.cagg().map(ParameterItem::CAggregate),
            _ => Err(error(
                self.tokens[self.index - 1].span,
                "unknown parameter item",
            )),
        }
    }

    fn result_item(&mut self) -> Result<ResultItem, TextParseError> {
        let kind = self.atom("expected result item")?;
        match kind.as_str() {
            "leaf" => self.parse_type(&[',', ')']).map(ResultItem::Leaf),
            "group" => self.group().map(ResultItem::Group),
            _ => Err(error(
                self.tokens[self.index - 1].span,
                "unknown result item",
            )),
        }
    }

    fn group(&mut self) -> Result<SignatureGroup, TextParseError> {
        let size = self.number64("invalid group size")?;
        let align = self.number64("invalid group alignment")?;
        let span = self.peek().span;
        validate_alignment(align, span)?;
        self.expect_punctuation('{')?;
        let members = self.list('}', |parser| {
            let offset = parser.number64("invalid group member offset")?;
            parser.expect_punctuation(':')?;
            let ty = parser.parse_type(&[',', '}'])?;
            Ok(SignatureMember { offset, ty })
        })?;
        validate_group(size, &members, span)?;
        Ok(SignatureGroup {
            size,
            align,
            members,
        })
    }

    fn cagg(&mut self) -> Result<CAggregateDescriptor, TextParseError> {
        let size = self.number64("invalid cagg size")?;
        let align = self.number64("invalid cagg alignment")?;
        let span = self.peek().span;
        validate_alignment(align, span)?;
        self.expect_punctuation('{')?;
        let members = self.list('}', |parser| {
            let offset = parser.number64("invalid cagg member offset")?;
            parser.expect_punctuation(':')?;
            let ty = parser.parse_type(&[',', '}', ':'])?;
            let bit_width = if parser.eat_punctuation(':') {
                Some(parser.number("invalid cagg bit-field width")?)
            } else {
                None
            };
            Ok(CAggregateMember {
                offset,
                ty,
                bit_width,
            })
        })?;
        let flags = self.flags()?;
        if members
            .windows(2)
            .any(|pair| pair[1].offset < pair[0].offset)
        {
            return Err(error(span, "cagg members must be in offset order"));
        }
        if members.iter().any(|member| member.offset > 64) {
            return Err(error(span, "cagg members beyond offset 64 must be omitted"));
        }
        Ok(CAggregateDescriptor {
            size,
            align,
            members,
            flags,
        })
    }

    fn parse_type(&mut self, stop: &[char]) -> Result<Type, TextParseError> {
        let start = self.index;
        let mut depth = [0i32; 4];
        while let Some(token) = self.tokens.get(self.index) {
            if let TokenKind::Punctuation(character) = token.kind {
                if depth == [0; 4] && stop.contains(&character) {
                    break;
                }
                let slot = match character {
                    '(' | ')' => Some(0),
                    '[' | ']' => Some(1),
                    '<' | '>' => Some(2),
                    '{' | '}' => Some(3),
                    _ => None,
                };
                if let Some(slot) = slot {
                    depth[slot] += if matches!(character, '(' | '[' | '<' | '{') {
                        1
                    } else {
                        -1
                    };
                }
            }
            self.index += 1;
        }
        if start == self.index {
            return Err(error(self.peek().span, "expected TVISA value type"));
        }
        let type_span = Span::new(
            self.tokens[start].span.start,
            self.tokens[self.index - 1].span.end,
        );
        let ty = parse_tvisa_type(&self.source[type_span.start..type_span.end]).map_err(
            |failure: TypeParseError| {
                error(
                    Span::new(
                        type_span.start + failure.span.start,
                        type_span.start + failure.span.end,
                    ),
                    &failure.message,
                )
            },
        )?;
        Ok(ty)
    }

    fn list<T>(
        &mut self,
        close: char,
        mut parse_item: impl FnMut(&mut Self) -> Result<T, TextParseError>,
    ) -> Result<Vec<T>, TextParseError> {
        let mut items = Vec::new();
        if self.eat_punctuation(close) {
            return Ok(items);
        }
        loop {
            items.push(parse_item(self)?);
            if self.eat_punctuation(close) {
                return Ok(items);
            }
            self.expect_punctuation(',')?;
        }
    }

    fn flags(&mut self) -> Result<Vec<String>, TextParseError> {
        if !self.eat_punctuation('[') {
            return Ok(Vec::new());
        }
        let flags = self.list(']', |parser| parser.atom("expected item flag"))?;
        if flags.is_empty() {
            return Err(error(self.peek().span, "flag list cannot be empty"));
        }
        Ok(flags)
    }

    fn number(&mut self, message: &str) -> Result<u32, TextParseError> {
        let token = self.peek().span;
        let value = self.atom(message)?;
        tir_format::numeric_literals::parse_unsigned_integer(&value)
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| error(token, message))
    }

    fn number64(&mut self, message: &str) -> Result<u64, TextParseError> {
        let token = self.peek().span;
        let value = self.atom(message)?;
        tir_format::numeric_literals::parse_unsigned_integer(&value)
            .and_then(|number| u64::try_from(number).ok())
            .ok_or_else(|| error(token, message))
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TextParseError> {
        let span = self.peek().span;
        if self.atom("expected identifier")? == expected {
            Ok(())
        } else {
            Err(error(span, "unexpected identifier"))
        }
    }

    fn atom(&mut self, message: &str) -> Result<String, TextParseError> {
        let token = self.next()?;
        match &token.kind {
            TokenKind::Atom(value) => Ok(value.clone()),
            _ => Err(error(token.span, message)),
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TextParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(error(
                self.peek().span,
                "unexpected punctuation in signature",
            ))
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Punctuation(value)) if *value == expected)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn eat_atom(&mut self, expected: &str) -> bool {
        if matches!(self.tokens.get(self.index).map(|token| &token.kind), Some(TokenKind::Atom(value)) if value == expected)
        {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect_arrow(&mut self) -> Result<(), TextParseError> {
        if matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::Arrow)
        ) {
            self.index += 1;
            Ok(())
        } else {
            Err(error(self.peek().span, "expected -> in signature"))
        }
    }

    fn next(&mut self) -> Result<&Token, TextParseError> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| error(self.peek().span, "unexpected end of signature"))?;
        self.index += 1;
        Ok(token)
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .expect("signature parser has at least one token")
    }
}

fn validate_alignment(align: u64, span: Span) -> Result<(), TextParseError> {
    if !align.is_power_of_two() {
        Err(error(span, "aggregate alignment must be a power of two"))
    } else {
        Ok(())
    }
}

fn validate_group(
    size: u64,
    members: &[SignatureMember],
    span: Span,
) -> Result<(), TextParseError> {
    if !(2..=4).contains(&members.len()) {
        return Err(error(
            span,
            "signature groups must have two to four members",
        ));
    }
    let mut ranges = Vec::with_capacity(members.len());
    for member in members {
        let Some((member_size, _)) = value_layout(&member.ty) else {
            return Err(error(
                span,
                "group members must be scalar or fixed-vector values",
            ));
        };
        if member_size > 16 {
            return Err(error(span, "group member values cannot exceed 16 bytes"));
        }
        let end = member
            .offset
            .checked_add(member_size)
            .ok_or_else(|| error(span, "group member range overflows"))?;
        if end > size {
            return Err(error(span, "group member lies beyond the aggregate size"));
        }
        ranges.push((member.offset, end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[1].0 < pair[0].1) {
        return Err(error(span, "group members overlap"));
    }
    Ok(())
}

fn value_layout(ty: &Type) -> Option<(u64, u64)> {
    match ty {
        Type::Integer(bits) => scalar_layout(u64::from(*bits)),
        Type::Float(kind) => scalar_layout(u64::from(kind.bits())),
        Type::Pointer(_)
        | Type::Code
        | Type::Texture { .. }
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::VisibleFunctionTable(_)
        | Type::Tensor { .. } => Some((8, 8)),
        Type::FixedVector { lanes, element } => {
            let (element_size, element_align) = value_layout(element)?;
            if matches!(element.as_ref(), Type::Integer(1)) {
                return Some((u64::from(*lanes), 1));
            }
            if matches!(
                element.as_ref(),
                Type::Float(FloatType::F6E2M3 | FloatType::F6E3M2 | FloatType::F4E2M1)
            ) {
                let Type::Float(kind) = element.as_ref() else {
                    return None;
                };
                let bits = u64::from(kind.bits());
                return Some((u64::from(*lanes).checked_mul(bits)?.div_ceil(8), 1));
            }
            let size = element_size.checked_mul(u64::from(*lanes))?;
            let power = u64::from(*lanes).checked_next_power_of_two()?;
            let align = element_size.checked_mul(power)?.min(16).max(element_align);
            Some((align_up(size, align)?, align))
        }
        _ => None,
    }
}

fn scalar_layout(bits: u64) -> Option<(u64, u64)> {
    if bits == 0 {
        return None;
    }
    let bytes = bits.div_ceil(8).max(1);
    let size = bytes.checked_next_power_of_two()?;
    Some((size, size.min(16)))
}

fn align_up(size: u64, align: u64) -> Option<u64> {
    let remainder = size % align;
    if remainder == 0 {
        Some(size)
    } else {
        size.checked_add(align - remainder)
    }
}
