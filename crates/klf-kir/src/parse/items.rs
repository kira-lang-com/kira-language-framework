//! Parsing of the module-level items of section 10.1.

use super::tables::parse_linkage;
use super::{
    ParseError, error, matching_close, split_once_required, split_top_level, symbol, take_quoted,
};
use crate::{
    Conformance, ExportRecord, GlobalDeclaration, GlobalInit, ImportRecord, ModuleAttribute,
    TraitDeclaration, TraitMethod, Type, Vtable, WitnessDeclaration,
};
use klf_core::Name;

pub(super) fn parse_attribute(offset: usize, line: &str) -> Result<ModuleAttribute, ParseError> {
    let rest = line
        .strip_prefix("attribute ")
        .ok_or_else(|| error(offset, "expected an `attribute` item"))?
        .trim();
    let Some((name, value)) = rest.split_once(" = ") else {
        return Ok(ModuleAttribute {
            name: rest.to_owned(),
            value: None,
        });
    };
    let (value, _) = take_quoted(value.trim(), offset)?;
    Ok(ModuleAttribute {
        name: name.trim().to_owned(),
        value: Some(value),
    })
}

pub(super) fn parse_global(offset: usize, line: &str) -> Result<GlobalDeclaration, ParseError> {
    let rest = line
        .strip_prefix("global ")
        .ok_or_else(|| error(offset, "expected a `global` item"))?
        .trim();
    let (linkage, rest) = match rest.split_once(' ') {
        Some((head, tail)) if matches!(head, "private" | "module" | "exported") => {
            (parse_linkage(head, offset)?, tail.trim_start())
        }
        _ => (crate::Linkage::Module, rest),
    };
    let (mutable, rest) = match rest.strip_prefix("const ") {
        Some(tail) => (false, tail.trim_start()),
        None => (true, rest),
    };
    let rest = rest
        .strip_prefix('@')
        .ok_or_else(|| error(offset, "global name must start with `@`"))?;
    let (name, rest) = split_once_required(rest, " : ", offset)?;
    let (ty_text, init_text) = split_once_required(rest, " = ", offset)?;
    let (init_text, thread_local) = match init_text.trim().strip_suffix(" tls") {
        Some(head) => (head.trim_end(), true),
        None => (init_text.trim(), false),
    };
    Ok(GlobalDeclaration {
        name: symbol(name, offset)?,
        linkage,
        ty: crate::type_parse::parse_type(ty_text.trim(), offset)?,
        init: parse_init(init_text, offset)?,
        mutable,
        thread_local,
    })
}

fn parse_init(text: &str, offset: usize) -> Result<GlobalInit, ParseError> {
    if text == "zero" {
        return Ok(GlobalInit::Zero);
    }
    if text.starts_with('"') {
        return Ok(GlobalInit::Bytes(parse_global_bytes(text, offset)?));
    }
    text.parse::<i128>().map(GlobalInit::Int).map_err(|_| {
        error(
            offset,
            "global initializer must be `zero`, an integer or a string",
        )
    })
}

fn parse_global_bytes(text: &str, offset: usize) -> Result<Vec<u8>, ParseError> {
    let body = text
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| error(offset, "string global must be quoted"))?;
    let mut bytes = Vec::new();
    let mut chars = body.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            let mut buffer = [0_u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('"') => bytes.push(b'"'),
            Some('\\') => bytes.push(b'\\'),
            Some(hex) => {
                let second = chars
                    .next()
                    .ok_or_else(|| error(offset, "string escape needs two hex digits"))?;
                let value = u8::from_str_radix(&format!("{hex}{second}"), 16)
                    .map_err(|_| error(offset, "invalid string escape"))?;
                bytes.push(value);
            }
            None => return Err(error(offset, "trailing backslash in string global")),
        }
    }
    Ok(bytes)
}

pub(super) fn parse_witness(offset: usize, line: &str) -> Result<WitnessDeclaration, ParseError> {
    let rest = line
        .strip_prefix("witness ")
        .ok_or_else(|| error(offset, "expected a `witness` item"))?;
    let (header, body) = split_body(rest, offset, "witness")?;
    let (ty_text, trait_text) = split_once_required(header, " : %", offset)?;
    let (witnesses_text, associated_text) = body
        .strip_prefix("witnesses: [")
        .and_then(|body| body.split_once("], associated: ["))
        .ok_or_else(|| error(offset, "witness declaration has invalid entries"))?;
    let associated_text = associated_text
        .strip_suffix(']')
        .ok_or_else(|| error(offset, "witness associated list is unclosed"))?;
    Ok(WitnessDeclaration {
        ty: crate::type_parse::parse_type(ty_text, offset)?,
        trait_name: Name::new(trait_text),
        witnesses: parse_symbols(witnesses_text, offset)?,
        associated: crate::type_parse::parse_type_list(associated_text, offset)?,
    })
}

pub(super) fn parse_vtable(offset: usize, line: &str) -> Result<Vtable, ParseError> {
    let rest = line
        .strip_prefix("vtable @")
        .ok_or_else(|| error(offset, "expected a `vtable` item"))?;
    let (header, body) = split_body(rest, offset, "vtable")?;
    let (name, rest) = split_once_required(header, " : ", offset)?;
    let (ty_text, trait_name) = match rest.split_once(" for %") {
        Some((ty, trait_name)) => (ty, Some(Name::new(trait_name.trim()))),
        None => (rest, None),
    };
    Ok(Vtable {
        name: symbol(name, offset)?,
        ty: crate::type_parse::parse_type(ty_text.trim(), offset)?,
        trait_name,
        entries: parse_symbols(body, offset)?,
    })
}

pub(super) fn parse_trait(offset: usize, line: &str) -> Result<TraitDeclaration, ParseError> {
    let rest = line
        .strip_prefix("trait %")
        .ok_or_else(|| error(offset, "expected a `trait` item"))?;
    let (header, body) = split_body(rest, offset, "trait")?;
    let (name, generics) = parse_generic_names(header, offset)?;
    let (associated_text, methods_text) = body
        .strip_prefix("associated: [")
        .and_then(|body| body.split_once("], methods: ["))
        .ok_or_else(|| error(offset, "trait declaration has invalid entries"))?;
    let methods_text = methods_text
        .strip_suffix(']')
        .ok_or_else(|| error(offset, "trait method list is unclosed"))?;
    Ok(TraitDeclaration {
        name,
        generics,
        associated: parse_generic_list(associated_text),
        methods: parse_methods(methods_text, offset)?,
    })
}

fn parse_methods(text: &str, offset: usize) -> Result<Vec<TraitMethod>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|entry| {
            let (name, signature) = split_once_required(entry, " : ", offset)?;
            let Type::Function(signature) =
                crate::type_parse::parse_type(signature.trim(), offset)?
            else {
                return Err(error(offset, "a trait method needs a function type"));
            };
            Ok(TraitMethod {
                name: Name::new(name.trim()),
                signature,
            })
        })
        .collect()
}

pub(super) fn parse_conformance(offset: usize, line: &str) -> Result<Conformance, ParseError> {
    let rest = line
        .strip_prefix("conformance ")
        .ok_or_else(|| error(offset, "expected a `conformance` item"))?
        .trim();
    let (rest, conditions) = match rest.split_once(" where ") {
        Some((head, conditions)) => (head, parse_conditions(conditions, offset)?),
        None => (rest, Vec::new()),
    };
    let (rest, witness) = match rest.split_once(" witness @") {
        Some((head, witness)) => (head, Some(symbol(witness, offset)?)),
        None => (rest, None),
    };
    let (ty_text, trait_name) = split_once_required(rest, " : %", offset)?;
    Ok(Conformance {
        ty: crate::type_parse::parse_type(ty_text.trim(), offset)?,
        trait_name: Name::new(trait_name.trim()),
        witness,
        conditions,
    })
}

fn parse_conditions(text: &str, offset: usize) -> Result<Vec<(Name, Name)>, ParseError> {
    split_top_level(text, ',')
        .into_iter()
        .map(|entry| {
            let (parameter, bound) = split_once_required(entry, ":", offset)?;
            Ok((
                Name::new(parameter.trim().trim_start_matches('$')),
                Name::new(bound.trim().trim_start_matches('%')),
            ))
        })
        .collect()
}

pub(super) fn parse_import(offset: usize, line: &str) -> Result<ImportRecord, ParseError> {
    let rest = line
        .strip_prefix("import @")
        .ok_or_else(|| error(offset, "expected an `import` item"))?;
    let (symbol_text, rest) = split_once_required(rest, " : ", offset)?;
    let (ty_text, module) = split_once_required(rest, " from ", offset)?;
    let (module, _) = take_quoted(module.trim(), offset)?;
    Ok(ImportRecord {
        symbol: symbol(symbol_text, offset)?,
        module,
        ty: crate::type_parse::parse_type(ty_text.trim(), offset)?,
    })
}

pub(super) fn parse_export(offset: usize, line: &str) -> Result<ExportRecord, ParseError> {
    let rest = line
        .strip_prefix("export @")
        .ok_or_else(|| error(offset, "expected an `export` item"))?;
    let (symbol_text, rest) = split_once_required(rest, " as ", offset)?;
    let (ridl_name, rest) = take_quoted(rest.trim(), offset)?;
    let ty_text = rest
        .trim_start()
        .strip_prefix(':')
        .ok_or_else(|| error(offset, "an export record needs `: <type>`"))?;
    Ok(ExportRecord {
        symbol: symbol(symbol_text, offset)?,
        ridl_name,
        ty: crate::type_parse::parse_type(ty_text.trim(), offset)?,
    })
}

/// Splits `header { body }` into its header and its trimmed body.
fn split_body<'a>(
    text: &'a str,
    offset: usize,
    what: &str,
) -> Result<(&'a str, &'a str), ParseError> {
    let (header, body) = text
        .split_once(" {")
        .ok_or_else(|| error(offset, format!("`{what}` body is missing")))?;
    let body = body
        .trim_end()
        .strip_suffix('}')
        .ok_or_else(|| error(offset, format!("`{what}` body is unclosed")))?;
    Ok((header, body.trim()))
}

fn parse_symbols(text: &str, offset: usize) -> Result<Vec<Name>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|name| {
            let name = name
                .trim()
                .strip_prefix('@')
                .filter(|name| !name.is_empty())
                .ok_or_else(|| error(offset, "a symbol entry must start with `@`"))?;
            symbol(name, offset)
        })
        .collect()
}

/// Parses `Name` or `Name<$T, $U>`.
pub(crate) fn parse_generic_names(
    text: &str,
    offset: usize,
) -> Result<(Name, Vec<Name>), ParseError> {
    let text = text.trim();
    let Some(open) = text.find('<') else {
        return Ok((Name::new(text), Vec::new()));
    };
    let close = matching_close(text, open, '<', '>')
        .ok_or_else(|| error(offset, "unclosed generic parameter list"))?;
    Ok((
        Name::new(text[..open].trim()),
        parse_generic_list(&text[open + 1..close]),
    ))
}

fn parse_generic_list(text: &str) -> Vec<Name> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|generic| Name::new(generic.trim().trim_start_matches('$')))
        .collect()
}
