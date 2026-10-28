//! Type declarations: appendix A.4 `typedecl` and its policy record.

use crate::parse::{ParseError, error, span, split_top_level, symbol};
use crate::type_parse::parse_type;
use crate::{
    CopyPolicy, DestroyPolicy, EnumVariant, Exclusivity, Field, LayoutRule, Pairing, Policy,
    RelocatePolicy, TypeDeclaration, TypeKind,
};
use klf_core::Name;

pub(crate) fn parse_type_declaration(
    declaration: &str,
    policy: &str,
    offset: usize,
) -> Result<TypeDeclaration, ParseError> {
    let rest = declaration
        .strip_prefix("type %")
        .ok_or_else(|| error(offset, "expected `type %Name`"))?;
    let (head, rest) = rest
        .split_once(" = ")
        .ok_or_else(|| error(offset, "type declaration is missing its kind"))?;
    let (name, generics) = split_generics(head, offset)?;
    let open = rest
        .find('{')
        .ok_or_else(|| error(offset, "type body is missing"))?;
    let close = rest
        .rfind('}')
        .ok_or_else(|| error(offset, "type body is unclosed"))?;
    let header = rest[..open].trim();
    let is_open = header.starts_with("open ");
    let (kind, layout) = parse_kind(header.strip_prefix("open ").unwrap_or(header), offset)?;
    let body = &rest[open + 1..close];
    let (fields, variants) = match kind {
        TypeKind::Enum => (Vec::new(), parse_variants(body, offset)?),
        TypeKind::Struct | TypeKind::Explicit => (parse_fields(body, offset)?, Vec::new()),
    };
    let (size, align) = parse_size_and_align(rest[close + 1..].trim(), offset)?;
    Ok(TypeDeclaration {
        name: Name::new(name),
        generics,
        kind,
        layout,
        fields,
        variants,
        open: is_open,
        size,
        align,
        policy: parse_policy(policy, offset)?,
        span: span(offset, declaration.len()),
    })
}

fn split_generics(head: &str, offset: usize) -> Result<(&str, Vec<Name>), ParseError> {
    let Some(open) = head.find('<') else {
        return Ok((head.trim(), Vec::new()));
    };
    let body = head
        .strip_suffix('>')
        .ok_or_else(|| error(offset, "generic parameter list is unclosed"))?;
    let generics = split_top_level(&body[open + 1..], ',')
        .into_iter()
        .map(|generic| {
            generic
                .trim()
                .strip_prefix('$')
                .map(Name::new)
                .ok_or_else(|| error(offset, "generic parameter must start with `$`"))
        })
        .collect::<Result<_, _>>()?;
    Ok((head[..open].trim(), generics))
}

fn parse_kind(text: &str, offset: usize) -> Result<(TypeKind, LayoutRule), ParseError> {
    let (kind, layout) = text
        .split_once(' ')
        .ok_or_else(|| error(offset, "type declaration is missing its layout"))?;
    let kind = match kind.trim() {
        "struct" => TypeKind::Struct,
        "enum" => TypeKind::Enum,
        "explicit" => TypeKind::Explicit,
        _ => {
            return Err(error(
                offset,
                "type kind must be `struct`, `enum` or `explicit`",
            ));
        }
    };
    let layout = match layout.trim() {
        "ridl" => LayoutRule::Ridl,
        "c" => LayoutRule::C,
        _ => return Err(error(offset, "type layout must be `ridl` or `c`")),
    };
    Ok((kind, layout))
}

fn parse_size_and_align(
    text: &str,
    offset: usize,
) -> Result<(Option<u64>, Option<u32>), ParseError> {
    let mut size = None;
    let mut align = None;
    let mut rest = text;
    while !rest.is_empty() {
        let (keyword, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        let (value, tail) = tail.split_once(' ').unwrap_or((tail, ""));
        match keyword {
            "size" => {
                size = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| error(offset, "type size must be an integer"))?,
                );
            }
            "align" => {
                align = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| error(offset, "type alignment must be an integer"))?,
                );
            }
            _ => return Err(error(offset, "expected `size` or `align`")),
        }
        rest = tail.trim();
    }
    Ok((size, align))
}

fn parse_variants(text: &str, offset: usize) -> Result<Vec<EnumVariant>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|variant| {
            let variant = variant.trim();
            let Some(open) = variant.find('(') else {
                return Ok(EnumVariant {
                    name: Name::new(variant),
                    payload: Vec::new(),
                });
            };
            let close = variant
                .strip_suffix(')')
                .map(|_| variant.len() - 1)
                .filter(|close| *close > open)
                .ok_or_else(|| error(offset, "enum payload is unclosed"))?;
            Ok(EnumVariant {
                name: Name::new(variant[..open].trim()),
                payload: crate::type_parse::parse_type_list(&variant[open + 1..close], offset)?,
            })
        })
        .collect()
}

fn parse_fields(text: &str, offset: usize) -> Result<Vec<Field>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|field| {
            let (name, rest) = field
                .trim()
                .split_once(':')
                .ok_or_else(|| error(offset, "struct field requires a name and type"))?;
            let (ty, offset_text) = match rest.rsplit_once(" @") {
                Some((ty, digits)) => (ty, Some(digits)),
                None => (rest, None),
            };
            let field_offset = offset_text
                .map(|digits| {
                    digits
                        .trim()
                        .parse::<u64>()
                        .map_err(|_| error(offset, "field offset must be an integer"))
                })
                .transpose()?;
            Ok(Field {
                name: Name::new(name.trim()),
                ty: parse_type(ty.trim(), offset)?,
                offset: field_offset,
            })
        })
        .collect()
}

fn parse_policy(text: &str, offset: usize) -> Result<Policy, ParseError> {
    let body = text
        .strip_prefix("policy {")
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| error(offset, "type declaration requires a `policy { ... }` line"))?;
    let mut values = std::collections::HashMap::new();
    for field in body.split(',') {
        let (key, value) = field
            .trim()
            .split_once(':')
            .ok_or_else(|| error(offset, "invalid policy field"))?;
        values.insert(key.trim(), value.trim());
    }
    Ok(Policy {
        copy: parse_copy(required(&values, "copy", offset)?, offset)?,
        destroy: parse_destroy(required(&values, "destroy", offset)?, offset)?,
        relocate: parse_relocate(required(&values, "relocate", offset)?, offset)?,
        exclusivity: match required(&values, "exclusivity", offset)? {
            "static" => Exclusivity::Static,
            "dynamic" => Exclusivity::Dynamic,
            "none" => Exclusivity::None,
            _ => return Err(error(offset, "invalid exclusivity policy")),
        },
        pairing: match required(&values, "pairing", offset)? {
            "counted" => Pairing::Counted,
            "none" => Pairing::None,
            _ => return Err(error(offset, "invalid pairing policy")),
        },
    })
}

fn required<'a>(
    values: &std::collections::HashMap<&str, &'a str>,
    name: &str,
    offset: usize,
) -> Result<&'a str, ParseError> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| error(offset, format!("policy is missing `{name}`")))
}

fn parse_copy(text: &str, offset: usize) -> Result<CopyPolicy, ParseError> {
    match text {
        "trivial" => Ok(CopyPolicy::Trivial),
        "none" => Ok(CopyPolicy::None),
        _ if text.starts_with("implicit(@") && text.ends_with(')') => Ok(CopyPolicy::Implicit(
            symbol(&text[10..text.len() - 1], offset)?,
        )),
        _ if text.starts_with("explicit(@") && text.ends_with(')') => Ok(CopyPolicy::Explicit(
            symbol(&text[10..text.len() - 1], offset)?,
        )),
        _ => Err(error(offset, "invalid copy policy")),
    }
}

fn parse_destroy(text: &str, offset: usize) -> Result<DestroyPolicy, ParseError> {
    match text {
        "trivial" => Ok(DestroyPolicy::Trivial),
        _ if text.starts_with("fn(@") && text.ends_with(')') => Ok(DestroyPolicy::Function(
            symbol(&text[4..text.len() - 1], offset)?,
        )),
        _ => Err(error(offset, "invalid destroy policy")),
    }
}

fn parse_relocate(text: &str, offset: usize) -> Result<RelocatePolicy, ParseError> {
    match text {
        "bitwise" => Ok(RelocatePolicy::Bitwise),
        _ if text.starts_with("fn(@") && text.ends_with(')') => Ok(RelocatePolicy::Function(
            symbol(&text[4..text.len() - 1], offset)?,
        )),
        _ => Err(error(offset, "invalid relocate policy")),
    }
}
