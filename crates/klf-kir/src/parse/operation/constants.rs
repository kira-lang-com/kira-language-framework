//! Sections 12.1 and 12.16: constants, references and type queries.

use super::common::{Parsed, parse_type, split_top_level_once};
use crate::Operation;
use crate::parse::{ParseError, error};
use klf_core::Name;

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_constant(text, offset)? {
        return Ok(Some(operation));
    }
    parse_reference(text, offset)
}

fn parse_constant(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("const.int ") {
        let (ty, value) = required_split(rest, ',', offset)?;
        Operation::ConstInt {
            ty: parse_type(ty, offset)?,
            value: value
                .trim()
                .parse()
                .map_err(|_| error(offset, "invalid integer literal"))?,
        }
    } else if let Some(value) = text.strip_prefix("const.bool ") {
        match value {
            "true" => Operation::ConstBool(true),
            "false" => Operation::ConstBool(false),
            _ => return Err(error(offset, "boolean literal must be `true` or `false`")),
        }
    } else if text == "const.null" {
        Operation::ConstNull
    } else if let Some(rest) = text.strip_prefix("const.float ") {
        let (ty, literal) = required_split(rest, ',', offset)?;
        Operation::ConstFloat {
            ty: parse_type(ty, offset)?,
            literal: literal.trim().to_owned(),
        }
    } else if let Some(ty) = text.strip_prefix("const.zero ") {
        Operation::ConstZero(parse_type(ty, offset)?)
    } else if let Some(literal) = text.strip_prefix("const.string ") {
        Operation::ConstString(unquote(literal, offset)?)
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_reference(text: &str, offset: usize) -> Parsed {
    for (prefix, thread_local) in [("global.addr @", false), ("tls.addr @", true)] {
        if let Some(global) = text.strip_prefix(prefix) {
            return Ok(Some(Operation::GlobalAddress {
                global: crate::parse::symbol(global, offset)?,
                thread_local,
            }));
        }
    }
    let operation = if let Some(function) = text.strip_prefix("func.ref @") {
        Operation::FunctionReference(crate::parse::symbol(function, offset)?)
    } else if let Some(rest) = text.strip_prefix("closure.callback ") {
        let (index, ty) = split_top_level_once(rest, ',')
            .ok_or_else(|| error(offset, "`closure.callback` needs `<index>, <fn type>`"))?;
        let user_data = index
            .trim()
            .parse()
            .map_err(|_| error(offset, "`closure.callback` index must be an integer"))?;
        let crate::Type::Function(ty) = parse_type(ty, offset)? else {
            return Err(error(offset, "`closure.callback` needs a function type"));
        };
        Operation::ClosureCallback { user_data, ty }
    } else if let Some(ty) = text.strip_prefix("descriptor.ref ") {
        Operation::DescriptorReference(parse_type(ty, offset)?)
    } else if let Some(rest) = text.strip_prefix("witness.ref ") {
        let (ty, trait_name) = required_split(rest, ',', offset)?;
        Operation::WitnessReference {
            ty: parse_type(ty, offset)?,
            trait_name: Name::new(trait_name.trim().trim_start_matches('%')),
        }
    } else if let Some(block) = text.strip_prefix("block.addr ") {
        Operation::BlockAddress(Name::new(block.trim()))
    } else if let Some(ty) = text.strip_prefix("type.size ") {
        Operation::TypeSize(parse_type(ty, offset)?)
    } else if let Some(ty) = text.strip_prefix("type.align ") {
        Operation::TypeAlign(parse_type(ty, offset)?)
    } else if let Some(ty) = text.strip_prefix("type.stride ") {
        Operation::TypeStride(parse_type(ty, offset)?)
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

pub(super) use crate::parse::unquote;

fn required_split(text: &str, delimiter: char, offset: usize) -> Result<(&str, &str), ParseError> {
    split_top_level_once(text, delimiter)
        .ok_or_else(|| error(offset, format!("expected `{delimiter}`")))
}

#[cfg(test)]
mod tests {
    use super::unquote;

    #[test]
    fn string_constants_roundtrip_escaped_utf8() {
        let original = "return \"a\\\"b\\\\c\\n\"\n\r\t\0\u{7}é🐈";
        assert_eq!(unquote(&format!("{original:?}"), 0).unwrap(), original);
        assert!(unquote(r#""\q""#, 0).is_err());
        assert!(unquote(r#""\u{d800}""#, 0).is_err());
        assert!(unquote(r#""a"b""#, 0).is_err());
    }
}
