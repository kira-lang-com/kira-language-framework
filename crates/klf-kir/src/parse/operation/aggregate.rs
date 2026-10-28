//! Sections 12.5 and 12.6: vectors, intrinsics and aggregates.

use super::common::{
    Parsed, parse_index, parse_result_suffix, parse_type, parse_values, split_call,
    split_top_level_once,
};
use super::constants::unquote;
use crate::Operation;
use crate::parse::{ParseError, error, matching_close, parse_value, split_top_level};

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_vector(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_structured(text, offset)? {
        return Ok(Some(operation));
    }
    parse_enum(text, offset)
}

fn parse_vector(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("vec.splat ") {
        let (ty, value) = required_split(rest, offset)?;
        Operation::VectorSplat {
            ty: parse_type(ty, offset)?,
            value: parse_value(value, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("vec.extract ") {
        let (vector, lane) = required_split(rest, offset)?;
        Operation::VectorExtract {
            vector: parse_value(vector, offset)?,
            lane: parse_index(lane, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("vec.insert ") {
        let parts = split_top_level(rest, ',');
        if parts.len() != 3 {
            return Err(error(offset, "`vec.insert` requires three operands"));
        }
        Operation::VectorInsert {
            vector: parse_value(parts[0], offset)?,
            lane: parse_index(parts[1], offset)?,
            value: parse_value(parts[2], offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("vec.shuffle ") {
        return parse_shuffle(rest, offset).map(Some);
    } else if let Some(rest) = text.strip_prefix("intrinsic ") {
        let (head, arguments, tail) = split_call(rest, offset, "intrinsic")?;
        Operation::Intrinsic(Box::new(crate::IntrinsicCall {
            name: unquote(head, offset)?,
            arguments,
            result: parse_result_suffix(tail, offset, "intrinsic")?,
        }))
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_shuffle(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let open = text
        .find('[')
        .ok_or_else(|| error(offset, "`vec.shuffle` requires a lane mask"))?;
    let close = matching_close(text, open, '[', ']')
        .ok_or_else(|| error(offset, "unclosed shuffle mask"))?;
    let (lhs, rhs) = required_split(text[..open].trim().trim_end_matches(','), offset)?;
    let body = text[open + 1..close].trim();
    let mask = if body.is_empty() {
        Vec::new()
    } else {
        split_top_level(body, ',')
            .into_iter()
            .map(|lane| parse_index(lane, offset))
            .collect::<Result<_, _>>()?
    };
    Ok(Operation::VectorShuffle {
        lhs: parse_value(lhs, offset)?,
        rhs: parse_value(rhs, offset)?,
        mask,
    })
}

fn parse_structured(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("struct.new ") {
        let (head, fields, _) = split_call(rest, offset, "struct.new")?;
        Operation::StructNew {
            ty: parse_type(head, offset)?,
            fields,
        }
    } else if let Some(rest) = text.strip_prefix("struct.extract ") {
        let (aggregate, field) = required_split(rest, offset)?;
        Operation::StructExtract {
            aggregate: parse_value(aggregate, offset)?,
            field: parse_index(field, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("struct.destructure ") {
        Operation::StructDestructure(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("tuple.new") {
        let open = rest
            .find('(')
            .ok_or_else(|| error(offset, "`tuple.new` requires an element list"))?;
        let close = matching_close(rest, open, '(', ')')
            .ok_or_else(|| error(offset, "unclosed tuple.new elements"))?;
        Operation::TupleNew(parse_values(&rest[open + 1..close], offset)?)
    } else if let Some(rest) = text.strip_prefix("tuple.extract ") {
        let (tuple, index) = required_split(rest, offset)?;
        Operation::TupleExtract {
            tuple: parse_value(tuple, offset)?,
            index: parse_index(index, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("tuple.destructure ") {
        Operation::TupleDestructure(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("array.new ") {
        let (head, elements, _) = split_call(rest, offset, "array.new")?;
        Operation::ArrayNew {
            element: parse_type(head, offset)?,
            elements,
        }
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_enum(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("enum.new ") {
        let (head, payload, _) = split_call(rest, offset, "enum.new")?;
        let (ty, variant) = required_split(head, offset)?;
        Operation::EnumNew {
            ty: parse_type(ty, offset)?,
            variant: parse_index(variant, offset)?,
            payload,
        }
    } else if let Some(rest) = text.strip_prefix("enum.tag ") {
        Operation::EnumTag {
            value: parse_value(rest, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("enum.take ") {
        let (value, variant) = required_split(rest, offset)?;
        Operation::EnumTake {
            value: parse_value(value, offset)?,
            variant: parse_index(variant, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("enum.project ") {
        let (value, variant) = required_split(rest, offset)?;
        Operation::EnumProject {
            value: parse_value(value, offset)?,
            variant: parse_index(variant, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("enum.payload.addr ") {
        let (base, variant) = required_split(rest, offset)?;
        Operation::EnumPayloadAddr {
            base: parse_value(base, offset)?,
            variant: parse_index(variant, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("enum.inject ") {
        let (address, variant) = required_split(rest, offset)?;
        Operation::EnumInject {
            address: parse_value(address, offset)?,
            variant: parse_index(variant, offset)?,
        }
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn required_split(text: &str, offset: usize) -> Result<(&str, &str), ParseError> {
    split_top_level_once(text, ',').ok_or_else(|| error(offset, "expected `,`"))
}
