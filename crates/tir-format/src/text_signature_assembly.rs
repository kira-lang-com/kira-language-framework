use crate::text::{Token, TokenKind, lex};
use crate::types::{
    BinaryEngine, BinaryFpMode, BinaryGenericStrategy, BinaryLinkage, BinarySignature, BinaryTarget,
};
use crate::{
    FunctionBody, TextItem, TextParseError, parse_function_body, parse_function_signature,
    parse_type,
};
use tir_diag::Span;
use tir_types::{Abi, FunctionType, ParameterType, Type};

pub(crate) struct ParsedFunction {
    pub(crate) record: BinarySignature,
    pub(crate) ty: Type,
    pub(crate) item: TextItem,
    pub(crate) body: Option<FunctionBody>,
}

pub(crate) fn parse_function(
    source: &str,
    item: TextItem,
) -> Result<ParsedFunction, TextParseError> {
    let tokens = lex(source)
        .map_err(|error| item_error(item, error.span, error.message))?
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect::<Vec<_>>();
    let (linkage, name, close) = parse_identity(&tokens, item)?;
    let body =
        parse_function_body(source).map_err(|error| item_error(item, error.span, error.message))?;
    let header_end = body
        .as_ref()
        .and_then(|body| {
            tokens
                .iter()
                .position(|token| punctuation(token, '{') && token.span.end == body.span.start)
        })
        .unwrap_or(tokens.len());
    let header_tokens = &tokens[..header_end];
    let signature = parse_function_signature(source)
        .map_err(|error| item_error(item, error.span, error.message))?
        .ok_or_else(|| item_error(item, Span::new(0, 0), "invalid function declaration"))?;
    let attributes_start = function_attributes_start(source, header_tokens, close, item)?;
    let attributes = parse_attributes(&header_tokens[attributes_start..], item)?;
    if attributes.target != BinaryTarget::Gpu
        && body.as_ref().is_some_and(|body| {
            body.blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| {
                    matches!(
                        instruction.opcode.as_str(),
                        "const.sampler"
                            | "gpu.builtin"
                            | "gpu.spec_const"
                            | "gpu.barrier"
                            | "gpu.memfence"
                    )
                })
        })
    {
        return Err(item_error(
            item,
            Span::new(0, 0),
            "GPU resource and synchronization operations require target(gpu) code",
        ));
    }
    if attributes.target != BinaryTarget::Gpu && attributes.engine.is_none() {
        return Err(item_error(
            item,
            tokens[0].span,
            "CPU functions require an explicit engine attribute",
        ));
    }

    let function_type = FunctionType {
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                context: parameter.context,
                convention: parameter.convention,
                ty: parameter.ty.clone(),
            })
            .collect(),
        result: Box::new(signature.result),
        failure: signature.failure.map(Box::new),
        asynchronous: attributes.asynchronous,
        abi: attributes.abi,
        foreign: false,
    };
    if let Some(body) = &body
        && !body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| {
                matches!(
                    instruction.opcode.as_str(),
                    "call"
                        | "tail.call"
                        | "func.ref"
                        | "global.addr"
                        | "tls.addr"
                        | "enum.tag"
                        | "struct.extract"
                        | "struct.destructure"
                        | "enum.take"
                        | "enum.project"
                        | "field.addr"
                        | "elem.addr"
                        | "enum.payload.addr"
                        | "enum.inject"
                        | "addr.toptr"
                        | "ptr.toaddr"
                        | "addr.cast"
                        | "const.aggregate"
                        | "gpu.builtin"
                        | "gpu.spec_const"
                        | "gpu.barrier"
                        | "gpu.memfence"
                )
            })
    {
        crate::text_function_body_assembly::validate_return_body(
            body,
            &function_type,
            source,
            item,
            None,
        )?;
    }
    let ty = Type::Function(function_type);
    let record = BinarySignature {
        name,
        linkage,
        function_type: 0,
        generic_parameters: Vec::new(),
        modifiers: attributes.modifiers,
        structured: attributes.structured,
        kernel_fp: attributes.kernel_fp,
        returns_twice: attributes.returns_twice,
        inlinable: attributes.inlinable,
        engine: attributes.engine,
        semantics: attributes.semantics,
        target: attributes.target,
        fp_mode: attributes.fp_mode,
        generic_strategy: attributes.generic_strategy,
        coroutine: None,
        instance_types: Vec::new(),
        variant: None,
        entry_kind: None,
        platform_export: None,
    };
    Ok(ParsedFunction {
        record,
        ty,
        item,
        body,
    })
}

struct FunctionAttributes {
    engine: Option<BinaryEngine>,
    semantics: Option<crate::SemanticsMode>,
    target: BinaryTarget,
    target_set: bool,
    abi: Abi,
    abi_set: bool,
    asynchronous: bool,
    modifiers: u64,
    structured: bool,
    kernel_fp: bool,
    returns_twice: bool,
    inlinable: bool,
    fp_mode: Option<BinaryFpMode>,
    generic_strategy: Option<BinaryGenericStrategy>,
}

impl Default for FunctionAttributes {
    fn default() -> Self {
        Self {
            engine: None,
            semantics: None,
            target: BinaryTarget::Cpu,
            target_set: false,
            abi: Abi::Ridl,
            abi_set: false,
            asynchronous: false,
            modifiers: 0,
            structured: false,
            kernel_fp: false,
            returns_twice: false,
            inlinable: false,
            fp_mode: None,
            generic_strategy: None,
        }
    }
}

fn parse_attributes(
    tokens: &[Token],
    item: TextItem,
) -> Result<FunctionAttributes, TextParseError> {
    let mut attributes = FunctionAttributes::default();
    let mut index = 0;
    while index < tokens.len() {
        if atom(&tokens[index]).is_none() {
            return Err(item_error(
                item,
                tokens[index].span,
                "invalid function attribute",
            ));
        }
        let name = atom(&tokens[index]).unwrap_or_default();
        let span = tokens[index].span;
        index += 1;
        match name {
            "engine" => {
                reject_duplicate(attributes.engine.is_some(), item, span, "engine")?;
                let value = attribute_atom(tokens, &mut index, item, "engine")?;
                attributes.engine = Some(match value.as_str() {
                    "native" => BinaryEngine::Native,
                    "runtime" => BinaryEngine::Runtime,
                    "any" => BinaryEngine::Any,
                    _ => return Err(item_error(item, span, "unknown function engine")),
                });
            }
            "semantics" => {
                reject_duplicate(attributes.semantics.is_some(), item, span, "semantics")?;
                attributes.semantics = Some(
                    match attribute_atom(tokens, &mut index, item, name)?.as_str() {
                        "defined" => crate::SemanticsMode::Defined,
                        "permissive" => crate::SemanticsMode::Permissive,
                        _ => return Err(item_error(item, span, "unknown function semantics mode")),
                    },
                );
            }
            "async" => {
                reject_duplicate(attributes.asynchronous, item, span, "async")?;
                attributes.asynchronous = true;
            }
            "abi" => {
                reject_duplicate(attributes.abi_set, item, span, "abi")?;
                attributes.abi_set = true;
                attributes.abi = match attribute_atom(tokens, &mut index, item, "abi")?.as_str() {
                    "ridl" => Abi::Ridl,
                    "c" => Abi::C,
                    _ => return Err(item_error(item, span, "unknown function ABI")),
                };
            }
            "target" => {
                reject_duplicate(attributes.target_set, item, span, "target")?;
                attributes.target_set = true;
                attributes.target = parse_target_attribute(tokens, &mut index, item, span)?;
            }
            "fp" => {
                reject_duplicate(attributes.fp_mode.is_some(), item, span, "fp")?;
                attributes.fp_mode = Some(
                    match attribute_atom(tokens, &mut index, item, name)?.as_str() {
                        "precise" => BinaryFpMode::Precise,
                        "fast" => BinaryFpMode::Fast,
                        "ieee_denormals" => BinaryFpMode::IeeeDenormals,
                        _ => return Err(item_error(item, span, "unknown floating-point mode")),
                    },
                );
            }
            "strategy" => {
                reject_duplicate(
                    attributes.generic_strategy.is_some(),
                    item,
                    span,
                    "strategy",
                )?;
                attributes.generic_strategy = Some(
                    match attribute_atom(tokens, &mut index, item, name)?.as_str() {
                        "specialize" => BinaryGenericStrategy::Specialize,
                        "polymorphic" => BinaryGenericStrategy::Polymorphic,
                        "both" => BinaryGenericStrategy::Both,
                        _ => return Err(item_error(item, span, "unknown generic strategy")),
                    },
                );
            }
            "structured" => {
                attributes.structured = set_once(attributes.structured, item, span, name)?
            }
            "returns_twice" => {
                attributes.returns_twice = set_once(attributes.returns_twice, item, span, name)?
            }
            "inlinable" => attributes.inlinable = set_once(attributes.inlinable, item, span, name)?,
            "cold" => {
                attributes.modifiers = set_modifier(attributes.modifiers, 41, item, span, name)?
            }
            "nounwind" => {
                attributes.modifiers = set_modifier(attributes.modifiers, 42, item, span, name)?
            }
            "noreturn" => {
                attributes.modifiers = set_modifier(attributes.modifiers, 43, item, span, name)?
            }
            "inline" => {
                attributes.modifiers = set_modifier(attributes.modifiers, 44, item, span, name)?
            }
            "noinline" => {
                attributes.modifiers = set_modifier(attributes.modifiers, 45, item, span, name)?
            }
            "kernel"
                if tokens
                    .get(index)
                    .is_some_and(|token| punctuation(token, '.'))
                    && tokens.get(index + 1).and_then(atom) == Some("fp") =>
            {
                reject_duplicate(attributes.kernel_fp, item, span, "kernel.fp")?;
                attributes.kernel_fp = true;
                index += 2;
            }
            _ => {
                return Err(item_error(
                    item,
                    span,
                    format!("text assembly does not yet support function attribute `{name}`"),
                ));
            }
        }
    }
    if attributes.modifiers & (1 << 44) != 0 && attributes.modifiers & (1 << 45) != 0 {
        return Err(item_error(
            item,
            tokens.last().map_or(Span::new(0, 0), |token| token.span),
            "function cannot be both inline and noinline",
        ));
    }
    Ok(attributes)
}

fn parse_identity(
    tokens: &[Token],
    item: TextItem,
) -> Result<(BinaryLinkage, String, usize), TextParseError> {
    let linkage = match tokens.get(1).and_then(atom) {
        Some("private") => BinaryLinkage::Private,
        Some("module") => BinaryLinkage::Module,
        Some("package") => BinaryLinkage::Package,
        Some("exported") => BinaryLinkage::Exported,
        _ => return Err(item_error(item, tokens[0].span, "unknown function linkage")),
    };
    let at = tokens
        .iter()
        .position(|token| punctuation(token, '@'))
        .ok_or_else(|| item_error(item, tokens[0].span, "function declaration has no name"))?;
    let open = tokens[at + 1..]
        .iter()
        .position(|token| punctuation(token, '('))
        .map(|index| at + 1 + index)
        .ok_or_else(|| {
            item_error(
                item,
                tokens[at].span,
                "function declaration has no parameters",
            )
        })?;
    let close = matching_paren(tokens, open).ok_or_else(|| {
        item_error(
            item,
            tokens[open].span,
            "function parameter list is not closed",
        )
    })?;
    let mut name = String::new();
    let mut expect_segment = true;
    for token in &tokens[at + 1..open] {
        match &token.kind {
            TokenKind::Atom(segment) if expect_segment => {
                name.push_str(segment);
                expect_segment = false;
            }
            TokenKind::Punctuation('.') if !expect_segment => {
                name.push('.');
                expect_segment = true;
            }
            _ => {
                return Err(item_error(
                    item,
                    token.span,
                    "invalid function name or generic parameters",
                ));
            }
        }
    }
    if name.is_empty() || expect_segment {
        return Err(item_error(
            item,
            tokens[at].span,
            "function name is empty or incomplete",
        ));
    }
    Ok((linkage, name, close))
}

fn function_attributes_start(
    source: &str,
    tokens: &[Token],
    close: usize,
    item: TextItem,
) -> Result<usize, TextParseError> {
    let arrow = tokens[close + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))
        .map(|index| close + 1 + index)
        .ok_or_else(|| {
            item_error(
                item,
                tokens[close].span,
                "function signature is missing '->'",
            )
        })?;
    let (index, _) = type_prefix(source, tokens, arrow + 1, item)?;
    if atom_at(tokens, index) == Some("fails") {
        type_prefix(source, tokens, index + 1, item).map(|(end, _)| end)
    } else {
        Ok(index)
    }
}

fn type_prefix(
    source: &str,
    tokens: &[Token],
    start: usize,
    item: TextItem,
) -> Result<(usize, Type), TextParseError> {
    for end in (start + 1..=tokens.len()).rev() {
        let Some(first) = tokens.get(start) else {
            break;
        };
        let Some(last) = tokens.get(end - 1) else {
            continue;
        };
        let Some(spelling) = source.get(first.span.start..last.span.end) else {
            continue;
        };
        if let Ok(ty) = parse_type(spelling) {
            return Ok((end, ty));
        }
    }
    Err(item_error(
        item,
        tokens
            .get(start)
            .map_or(Span::new(0, 0), |token| token.span),
        "invalid function result type",
    ))
}

fn attribute_atom(
    tokens: &[Token],
    index: &mut usize,
    item: TextItem,
    name: &str,
) -> Result<String, TextParseError> {
    if !tokens
        .get(*index)
        .is_some_and(|token| punctuation(token, '('))
    {
        return Err(item_error(
            item,
            token_span(tokens, *index),
            format!("{name} requires parentheses"),
        ));
    }
    *index += 1;
    let value = atom_at(tokens, *index)
        .ok_or_else(|| {
            item_error(
                item,
                token_span(tokens, *index),
                format!("invalid {name} value"),
            )
        })?
        .to_owned();
    *index += 1;
    if !tokens
        .get(*index)
        .is_some_and(|token| punctuation(token, ')'))
    {
        return Err(item_error(
            item,
            token_span(tokens, *index),
            format!("invalid {name} attribute"),
        ));
    }
    *index += 1;
    Ok(value)
}

fn parse_target_attribute(
    tokens: &[Token],
    index: &mut usize,
    item: TextItem,
    span: Span,
) -> Result<BinaryTarget, TextParseError> {
    if !tokens
        .get(*index)
        .is_some_and(|token| punctuation(token, '('))
    {
        return Err(item_error(item, span, "target requires parentheses"));
    }
    *index += 1;
    let first = parse_target_name(atom_at(tokens, *index), item, token_span(tokens, *index))?;
    *index += 1;
    let second = if tokens
        .get(*index)
        .is_some_and(|token| punctuation(token, ','))
    {
        *index += 1;
        let second = parse_target_name(atom_at(tokens, *index), item, token_span(tokens, *index))?;
        *index += 1;
        Some(second)
    } else {
        None
    };
    if !tokens
        .get(*index)
        .is_some_and(|token| punctuation(token, ')'))
    {
        return Err(item_error(
            item,
            token_span(tokens, *index),
            "invalid target attribute",
        ));
    }
    *index += 1;
    match (first, second) {
        (BinaryTarget::Cpu, None) => Ok(BinaryTarget::Cpu),
        (BinaryTarget::Gpu, None) => Ok(BinaryTarget::Gpu),
        (BinaryTarget::Cpu, Some(BinaryTarget::Gpu))
        | (BinaryTarget::Gpu, Some(BinaryTarget::Cpu)) => Ok(BinaryTarget::Both),
        _ => Err(item_error(item, span, "target must name cpu, gpu, or both")),
    }
}

fn parse_target_name(
    value: Option<&str>,
    item: TextItem,
    span: Span,
) -> Result<BinaryTarget, TextParseError> {
    match value {
        Some("cpu") => Ok(BinaryTarget::Cpu),
        Some("gpu") => Ok(BinaryTarget::Gpu),
        _ => Err(item_error(item, span, "unknown function target")),
    }
}

fn set_once(value: bool, item: TextItem, span: Span, name: &str) -> Result<bool, TextParseError> {
    reject_duplicate(value, item, span, name)?;
    Ok(true)
}

fn set_modifier(
    modifiers: u64,
    bit: u32,
    item: TextItem,
    span: Span,
    name: &str,
) -> Result<u64, TextParseError> {
    if modifiers & (1 << bit) != 0 {
        return Err(item_error(
            item,
            span,
            format!("duplicate `{name}` attribute"),
        ));
    }
    Ok(modifiers | (1 << bit))
}

fn reject_duplicate(
    duplicate: bool,
    item: TextItem,
    span: Span,
    name: &str,
) -> Result<(), TextParseError> {
    if duplicate {
        return Err(item_error(
            item,
            span,
            format!("duplicate `{name}` attribute"),
        ));
    }
    Ok(())
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0_u32;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation('(') => depth = depth.checked_add(1)?,
            TokenKind::Punctuation(')') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(actual) if actual == expected)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn token_span(tokens: &[Token], index: usize) -> Span {
    tokens
        .get(index)
        .or_else(|| tokens.last())
        .map_or(Span::new(0, 0), |token| token.span)
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
