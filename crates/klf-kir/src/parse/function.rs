//! Parsing of functions, blocks and instructions (sections 10.3 and 10.4).

use super::operation::parse_operation;
use super::tables::parse_linkage;
use super::{
    Lines, ParseError, error, find_unquoted, matching_close, parse_value, span, split_top_level,
    symbol,
};
use crate::{
    Block, CPrototype, CType, Convention, Engine, Function, FunctionAbi, FunctionAttribute,
    GenericParameter, Instruction, Parameter, Type,
};
use klf_core::Name;

pub(super) fn parse_function(
    offset: usize,
    line: &str,
    lines: &mut Lines<'_>,
) -> Result<Function, ParseError> {
    let (declaration, has_body) = match line.strip_suffix('{') {
        Some(head) => (head.trim_end(), true),
        None => (line.trim_end(), false),
    };
    let rest = declaration
        .strip_prefix("func ")
        .ok_or_else(|| error(offset, "expected a `func` item"))?;
    let (linkage_text, rest) = rest
        .split_once(' ')
        .ok_or_else(|| error(offset, "function linkage is missing"))?;
    let linkage = parse_linkage(linkage_text, offset)?;
    let (foreign, rest) = match rest.strip_prefix("foreign ") {
        Some(after) => (true, after),
        None => (false, rest),
    };
    let rest = rest
        .strip_prefix('@')
        .ok_or_else(|| error(offset, "function name must start with `@`"))?;
    let open =
        find_unquoted(rest, '(').ok_or_else(|| error(offset, "function parameters are missing"))?;
    let (name, generics) = parse_generic_header(&rest[..open], offset)?;
    let close = matching_close(rest, open, '(', ')')
        .ok_or_else(|| error(offset, "unclosed parameter list"))?;
    let parameters = parse_parameters(&rest[open + 1..close], offset)?;
    let signature = Signature::parse(&rest[close + 1..], offset)?;
    if foreign && has_body {
        return Err(error(offset, "a foreign function declaration has no body"));
    }
    Ok(Function {
        name,
        linkage,
        abi: signature.abi,
        nounwind: signature.nounwind,
        foreign,
        c_prototype: signature.c_prototype,
        generics,
        attributes: signature.attributes,
        parameters,
        result: signature.result,
        failure: signature.failure,
        engine: signature.engine,
        blocks: if has_body {
            parse_blocks(lines)?
        } else {
            Vec::new()
        },
        span: span(offset, line.len()),
    })
}

/// The `-> R [fails E] engine(...) [attributes]` tail of a function header.
struct Signature {
    result: Type,
    failure: Option<Type>,
    engine: Engine,
    abi: FunctionAbi,
    nounwind: bool,
    attributes: Vec<FunctionAttribute>,
    c_prototype: Option<CPrototype>,
}

impl Signature {
    fn parse(text: &str, offset: usize) -> Result<Self, ParseError> {
        let after = text
            .trim()
            .strip_prefix("-> ")
            .ok_or_else(|| error(offset, "function result type is missing"))?;
        let engine_start = after
            .rfind(" engine(")
            .ok_or_else(|| error(offset, "function engine is missing"))?;
        let engine_rest = &after[engine_start + " engine(".len()..];
        let engine_close = engine_rest
            .find(')')
            .ok_or_else(|| error(offset, "unclosed engine attribute"))?;
        let engine = match &engine_rest[..engine_close] {
            "runtime" => Engine::Runtime,
            "native" => Engine::Native,
            "any" => Engine::Any,
            _ => return Err(error(offset, "unknown function engine")),
        };
        let mut nounwind = false;
        let mut abi = FunctionAbi::Platform;
        let mut abi_seen = false;
        let mut attributes = Vec::new();
        let mut tail = &engine_rest[engine_close + 1..];
        let mut c_prototype = None;
        if let Some(start) = tail.find(" c(") {
            let open = start + " c".len();
            let close = matching_close(tail, open, '(', ')')
                .ok_or_else(|| error(offset, "unclosed C prototype"))?;
            c_prototype = Some(parse_c_prototype(&tail[open + 1..close], offset)?);
            if !tail[close + 1..].trim().is_empty() {
                return Err(error(offset, "the C prototype ends a function header"));
            }
            tail = &tail[..start];
        }
        for token in tail.split_whitespace() {
            if token == "nounwind" {
                nounwind = true;
            } else if token == "abi" || token.starts_with("abi(") {
                if abi_seen {
                    return Err(error(offset, "duplicate function ABI token"));
                }
                abi = parse_abi(token, offset)?;
                abi_seen = true;
            } else if let Some(attribute) = FunctionAttribute::parse(token) {
                attributes.push(attribute);
            } else {
                return Err(error(
                    offset,
                    format!("unknown function attribute `{token}`"),
                ));
            }
        }
        let head = after[..engine_start].trim();
        let (result_text, failure_text) = head
            .split_once(" fails ")
            .map_or((head, None), |(result, failure)| (result, Some(failure)));
        Ok(Self {
            result: crate::type_parse::parse_type(result_text, offset)?,
            failure: failure_text
                .map(|value| crate::type_parse::parse_type(value, offset))
                .transpose()?,
            engine,
            abi,
            nounwind,
            attributes,
            c_prototype,
        })
    }
}

/// `[syscall name:] t1, t2 -> r`, the inside of a function's `c(...)`
/// clause.
fn parse_c_prototype(text: &str, offset: usize) -> Result<CPrototype, ParseError> {
    let mut syscall = None;
    let mut text = text;
    if let Some(rest) = text.trim_start().strip_prefix("syscall ") {
        let (name, rest) = rest
            .split_once(':')
            .ok_or_else(|| error(offset, "a system call's name ends with `:`"))?;
        syscall = Some(Name::new(name.trim()));
        text = rest;
    }
    let (parameters, result) = text
        .rsplit_once("->")
        .ok_or_else(|| error(offset, "a C prototype names its result after `->`"))?;
    let mut types = Vec::new();
    let mut variadic = None;
    if !parameters.trim().is_empty() {
        for parameter in split_top_level(parameters, ',') {
            match parameter.trim() {
                "..." if variadic.is_none() => variadic = Some(types.len()),
                "..." => return Err(error(offset, "a C prototype is variadic once")),
                text => types.push(parse_c_type(text, offset)?),
            }
        }
    }
    Ok(CPrototype {
        parameters: types,
        result: parse_c_type(result.trim(), offset)?,
        variadic,
        syscall,
    })
}

fn parse_c_type(text: &str, offset: usize) -> Result<CType, ParseError> {
    let width = |digits: &str| digits.parse::<u16>().ok();
    let ty = match text {
        "void" => CType::Void,
        "bool" => CType::Bool,
        "ptr" => CType::Pointer,
        _ if text.starts_with('%') => match crate::type_parse::parse_type(text, offset)? {
            Type::Nominal(name, arguments) if arguments.is_empty() => CType::Value(name),
            _ => return Err(error(offset, format!("unknown C type `{text}`"))),
        },
        _ => match (text.get(..1), text.get(1..).and_then(width)) {
            (Some("s"), Some(bits)) => CType::Signed(bits),
            (Some("u"), Some(bits)) => CType::Unsigned(bits),
            (Some("f"), Some(bits)) => CType::Float(bits),
            _ => return Err(error(offset, format!("unknown C type `{text}`"))),
        },
    };
    Ok(ty)
}

fn parse_abi(token: &str, offset: usize) -> Result<FunctionAbi, ParseError> {
    let value = token
        .strip_prefix("abi(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| error(offset, format!("malformed function ABI token `{token}`")))?;
    match value {
        "platform" => Ok(FunctionAbi::Platform),
        "ridl" => Ok(FunctionAbi::Ridl),
        _ => Err(error(offset, format!("unknown function ABI `{value}`"))),
    }
}

fn parse_blocks(lines: &mut Lines<'_>) -> Result<Vec<Block>, ParseError> {
    let mut blocks = Vec::new();
    loop {
        let (offset, line) = lines.next_required("function body or `}`")?;
        if line == "}" {
            return Ok(blocks);
        }
        if !line.ends_with(':') {
            return Err(error(offset, "expected a basic block label"));
        }
        blocks.push(parse_block(offset, line, lines)?);
    }
}

/// Parses `name<$T: %Bound, $U>` from a function header.
fn parse_generic_header(
    text: &str,
    offset: usize,
) -> Result<(Name, Vec<GenericParameter>), ParseError> {
    let Some(open) = find_unquoted(text, '<') else {
        return Ok((symbol(text, offset)?, Vec::new()));
    };
    let close = matching_close(text, open, '<', '>')
        .ok_or_else(|| error(offset, "unclosed generic parameter list"))?;
    let generics = split_top_level(&text[open + 1..close], ',')
        .into_iter()
        .map(parse_generic_parameter)
        .collect();
    Ok((symbol(&text[..open], offset)?, generics))
}

fn parse_generic_parameter(text: &str) -> GenericParameter {
    let (name, bounds) = match text.split_once(':') {
        Some((name, bounds)) => (
            name,
            bounds
                .split('+')
                .map(|bound| Name::new(bound.trim().trim_start_matches('%')))
                .collect(),
        ),
        None => (text, Vec::new()),
    };
    GenericParameter {
        name: Name::new(name.trim().trim_start_matches('$')),
        bounds,
    }
}

fn parse_block(offset: usize, line: &str, lines: &mut Lines<'_>) -> Result<Block, ParseError> {
    let header = line
        .strip_suffix(':')
        .ok_or_else(|| error(offset, "expected a basic block label"))?;
    let (name, parameters) = if let Some(open) = header.find('(') {
        let close = matching_close(header, open, '(', ')')
            .ok_or_else(|| error(offset, "unclosed block parameter list"))?;
        if close + 1 != header.len() {
            return Err(error(offset, "unexpected text after block parameters"));
        }
        (
            Name::new(&header[..open]),
            parse_parameters(&header[open + 1..close], offset)?,
        )
    } else {
        (Name::new(header), Vec::new())
    };
    let mut instructions = Vec::new();
    while let Some((instruction_offset, instruction_line)) = lines.peek() {
        if instruction_line == "}" || instruction_line.ends_with(':') {
            break;
        }
        lines.next();
        instructions.push(parse_instruction(instruction_offset, instruction_line)?);
    }
    Ok(Block {
        name,
        parameters,
        instructions,
        span: span(offset, line.len()),
    })
}

fn parse_instruction(offset: usize, line: &str) -> Result<Instruction, ParseError> {
    let (line, debug_location) = crate::debug::parse_debug_location(line, offset)?;
    let (results, operation_text) = split_results(line, offset)?;
    Ok(Instruction {
        results,
        operation: parse_operation(operation_text, offset)?,
        debug_location: debug_location.map(Box::new),
        span: span(offset, line.len()),
    })
}

fn split_results(line: &str, offset: usize) -> Result<(Vec<Name>, &str), ParseError> {
    if line.starts_with('%') {
        let (result, operation) = line
            .split_once(" = ")
            .ok_or_else(|| error(offset, "result definition is missing `=`"))?;
        return Ok((vec![Name::new(result.trim_start_matches('%'))], operation));
    }
    if line.starts_with('(') {
        let (results, operation) = line
            .split_once(" = ")
            .ok_or_else(|| error(offset, "result definition is missing `=`"))?;
        let body = results
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| error(offset, "invalid multiple-result list"))?;
        let names = split_top_level(body, ',')
            .into_iter()
            .map(|value| parse_value(value, offset))
            .collect::<Result<_, _>>()?;
        return Ok((names, operation));
    }
    Ok((Vec::new(), line))
}

pub(crate) fn parse_parameters(text: &str, offset: usize) -> Result<Vec<Parameter>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|parameter| parse_parameter(parameter, offset))
        .collect()
}

fn parse_parameter(text: &str, offset: usize) -> Result<Parameter, ParseError> {
    let (name, rest) = super::split_once_required(text, ":", offset)?;
    let name = parse_value(name, offset)?;
    let rest = rest.trim();
    let (context, rest) = match rest.strip_prefix("context ") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, rest),
    };
    let (convention, ty) = if let Some(ty) = rest.strip_prefix("owned ") {
        (Some(Convention::Owned), ty)
    } else if let Some(ty) = rest.strip_prefix("borrowmut ") {
        (Some(Convention::BorrowMut), ty)
    } else if let Some(ty) = rest.strip_prefix("borrow ") {
        (Some(Convention::Borrow), ty)
    } else {
        (None, rest)
    };
    Ok(Parameter {
        name,
        convention,
        context,
        ty: crate::type_parse::parse_type(ty, offset)?,
        span: span(offset, text.len()),
    })
}
