use crate::parse::{ParseError, error};
use crate::{Convention, FunctionAbi, FunctionType, ParameterType, Type};
use klf_core::Name;

pub(crate) fn parse_type(text: &str, offset: usize) -> Result<Type, ParseError> {
    let text = text.trim();
    match text {
        "ptr" => return Ok(Type::Pointer),
        "unit" => return Ok(Type::Unit),
        "never" => return Ok(Type::Never),
        _ => {}
    }
    if let Some(bits) = scalar_width(text, 'i')
        && matches!(bits, 1 | 8 | 16 | 32 | 64 | 128)
    {
        return Ok(Type::Integer(bits));
    }
    if let Some(bits) = scalar_width(text, 'f')
        && matches!(bits, 32 | 64)
    {
        return Ok(Type::Float(bits));
    }
    for (prefix, constructor) in [
        ("ref ", Type::Reference as fn(Box<Type>) -> Type),
        ("uniq ", Type::Unique),
        ("weak ", Type::Weak),
    ] {
        if let Some(inner) = text.strip_prefix(prefix) {
            return Ok(constructor(Box::new(parse_type(inner, offset)?)));
        }
    }
    if let Some(inner) = text.strip_prefix('*') {
        return Ok(Type::Address(Box::new(parse_type(inner, offset)?)));
    }
    if let Some(name) = text.strip_prefix('$') {
        return Ok(Type::Generic(Name::new(name)));
    }
    if let Some(nominal) = text.strip_prefix('%') {
        return parse_nominal(nominal, offset);
    }
    if let Some(inner) = delimited(text, '[', ']') {
        let (count, element) = required_split(inner, " x ", offset)?;
        return Ok(Type::Array(
            count
                .parse()
                .map_err(|_| error(offset, "invalid array length"))?,
            Box::new(parse_type(element, offset)?),
        ));
    }
    if let Some(inner) = text
        .strip_prefix("vec<")
        .and_then(|value| value.strip_suffix('>'))
    {
        let (count, element) = required_split(inner, " x ", offset)?;
        return Ok(Type::Vector(
            count
                .parse()
                .map_err(|_| error(offset, "invalid vector length"))?,
            Box::new(parse_type(element, offset)?),
        ));
    }
    if let Some(rest) = text.strip_prefix("fn(") {
        return parse_function(rest, offset);
    }
    if let Some(inner) = delimited(text, '(', ')') {
        return Ok(Type::Tuple(parse_type_list(inner, offset)?));
    }
    Err(error(offset, format!("unknown KIR type `{text}`")))
}

fn parse_nominal(text: &str, offset: usize) -> Result<Type, ParseError> {
    if let Some(open) = text.find('<') {
        let arguments = text[open + 1..]
            .strip_suffix('>')
            .ok_or_else(|| error(offset, "unclosed nominal type arguments"))?;
        Ok(Type::Nominal(
            Name::new(&text[..open]),
            parse_type_list(arguments, offset)?,
        ))
    } else {
        Ok(Type::Nominal(Name::new(text), Vec::new()))
    }
}

fn parse_function(text: &str, offset: usize) -> Result<Type, ParseError> {
    let close = matching_close(text).ok_or_else(|| error(offset, "unclosed function type"))?;
    let parameters = parse_parameter_types(&text[..close], offset)?;
    let result_text = text[close + 1..]
        .trim()
        .strip_prefix("-> ")
        .ok_or_else(|| error(offset, "function type is missing result"))?;
    let (result_text, abi) = parse_function_abi(result_text, offset)?;
    let (result, failure) = result_text
        .split_once(" fails ")
        .map_or((result_text, None), |(result, failure)| {
            (result, Some(failure))
        });
    Ok(Type::Function(FunctionType {
        parameters,
        result: Box::new(parse_type(result, offset)?),
        failure: failure
            .map(|value| parse_type(value, offset).map(Box::new))
            .transpose()?,
        abi,
    }))
}

fn parse_function_abi(text: &str, offset: usize) -> Result<(&str, FunctionAbi), ParseError> {
    let text = text.trim_end();
    let Some(separator) = text.rfind(char::is_whitespace) else {
        if text.starts_with("abi") {
            return Err(error(
                offset,
                format!("malformed function type ABI token `{text}`"),
            ));
        }
        return Ok((text, FunctionAbi::Platform));
    };
    let separator_end = separator + text[separator..].chars().next().map_or(0, char::len_utf8);
    let token = &text[separator_end..];
    if !token.starts_with("abi") {
        return Ok((text, FunctionAbi::Platform));
    }
    let value = token
        .strip_prefix("abi(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| {
            error(
                offset,
                format!("malformed function type ABI token `{token}`"),
            )
        })?;
    let abi = match value {
        "platform" => FunctionAbi::Platform,
        "ridl" => FunctionAbi::Ridl,
        _ => {
            return Err(error(
                offset,
                format!("unknown function type ABI `{value}`"),
            ));
        }
    };
    Ok((text[..separator].trim_end(), abi))
}

pub(crate) fn parse_type_list(text: &str, offset: usize) -> Result<Vec<Type>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|value| parse_type(value, offset))
        .collect()
}

fn parse_parameter_types(text: &str, offset: usize) -> Result<Vec<ParameterType>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|value| {
            let value = value.trim();
            let (context, value) = if let Some(value) = value.strip_prefix("context ") {
                (true, value.trim_start())
            } else {
                (false, value)
            };
            let (convention, ty) = if let Some(ty) = value.strip_prefix("owned ") {
                (Some(Convention::Owned), ty)
            } else if let Some(ty) = value.strip_prefix("borrowmut ") {
                (Some(Convention::BorrowMut), ty)
            } else if let Some(ty) = value.strip_prefix("borrow ") {
                (Some(Convention::Borrow), ty)
            } else {
                (None, value)
            };
            Ok(ParameterType {
                context,
                convention,
                ty: parse_type(ty, offset)?,
            })
        })
        .collect()
}

fn scalar_width(text: &str, prefix: char) -> Option<u16> {
    text.strip_prefix(prefix)?.parse().ok()
}

fn delimited(text: &str, open: char, close: char) -> Option<&str> {
    text.strip_prefix(open)?.strip_suffix(close)
}

fn required_split<'a>(
    text: &'a str,
    delimiter: &str,
    offset: usize,
) -> Result<(&'a str, &'a str), ParseError> {
    text.split_once(delimiter)
        .ok_or_else(|| error(offset, format!("expected `{delimiter}`")))
}

fn matching_close(text: &str) -> Option<usize> {
    let mut depth = 1_u32;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(text: &str, delimiter: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        match character {
            '(' | '[' | '<' => depth += 1,
            '>' if index > 0 && text.as_bytes()[index - 1] == b'-' => {}
            ')' | ']' | '>' => depth -= 1,
            value if value == delimiter && depth == 0 => {
                result.push(text[start..index].trim());
                start = index + value.len_utf8();
            }
            _ => {}
        }
    }
    result.push(text[start..].trim());
    result
}
