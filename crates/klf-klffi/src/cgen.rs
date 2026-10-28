//! `klf ffi gen` (section 26.5): preprocess a C header with the target's
//! predefined macros and the include directories, then read the `.klffi`
//! declarations of the function prototypes, structs, enums, typedefs and
//! integer/string `#define`s it contains (see `crate::preprocess`). Anything
//! the declaration parser cannot map is left out with a remark.

use std::collections::BTreeSet;
use std::path::Path;

use crate::cdecl::{Known, declaration, top_level};
use crate::names::{parameter_name, unbindable_name};
use crate::model::{
    Annotation, AnnotationKind, FfiConstant, FfiFunction, FfiItem, FfiModule, FfiStruct, FfiType,
};
use crate::preprocess::{
    Define, Environment, HeaderError, Preprocessed, integer_literal, preprocess,
};
use crate::preprocess::{Fault, Kind, Token, render, tokenize};

/// One line of explanation for a header construct that was not translated.
pub type Remark = String;

/// Generate a `.klffi` module from the C header at `path` whose text is
/// `header`. `library` names the linked library recorded on each
/// `@FFI.Extern`, `environment` gives the predefined macros and include
/// directories, and `read` returns the text of an included file.
///
/// # Errors
/// Returns [`HeaderError`] at the first malformed preprocessor directive.
pub fn generate(
    header: &str,
    path: &Path,
    library: &str,
    environment: Environment<'_>,
    read: &mut dyn FnMut(&Path) -> Option<String>,
) -> Result<(FfiModule, Vec<Remark>), HeaderError> {
    generate_withholding(header, path, library, environment, &[], read)
}

/// [`generate`], with the `@FFI.Pointer` declarations of the names in `withheld`
/// left out. A package whose other headers already declare a pointer type (such
/// as `float_ptr`) passes its name, so it is not declared twice; every use still
/// names it.
///
/// # Errors
/// Returns [`HeaderError`] at the first malformed preprocessor directive.
pub fn generate_withholding(
    header: &str,
    path: &Path,
    library: &str,
    environment: Environment<'_>,
    withheld: &[String],
    read: &mut dyn FnMut(&Path) -> Option<String>,
) -> Result<(FfiModule, Vec<Remark>), HeaderError> {
    let Preprocessed {
        text,
        defines,
        mut remarks,
    } = preprocess(header, path, environment, read)?;
    let code = without_decorations(&text).map_err(|fault| HeaderError {
        path: path.to_path_buf(),
        line: fault.line,
        message: fault.message,
    })?;
    let mut items: Vec<FfiItem> = defines
        .iter()
        .filter_map(|define| constant(define).map(FfiItem::Constant))
        .collect();

    let mut known = Known::default();
    known.withheld = withheld.iter().cloned().collect();
    for statement in top_level(&code) {
        match declaration(&statement, &mut known) {
            Some(Ok(declared)) => items.extend(declared),
            Some(Err(remark)) => remarks.push(remark),
            None => match prototype(&statement, library, &mut known) {
                Ok(Some(function)) => items.push(FfiItem::Function(function)),
                Ok(None) => {}
                Err(remark) => remarks.push(remark),
            },
        }
        items.append(&mut known.pending);
    }

    let mut aliases = alias_declarations(&items);
    aliases.append(&mut items);
    Ok((FfiModule { items: settle(aliases) }, remarks))
}

/// Drops the `@FFI.Alias` of an opaque tag that a struct body defines, and
/// all but the first alias of each tag.
fn settle(items: Vec<FfiItem>) -> Vec<FfiItem> {
    let defined: BTreeSet<String> = items
        .iter()
        .filter_map(|item| match item {
            FfiItem::Struct(structure)
                if structure
                    .annotations
                    .iter()
                    .any(|annotation| annotation.kind == AnnotationKind::Struct) =>
            {
                Some(structure.name.clone())
            }
            _ => None,
        })
        .collect();
    let mut aliased = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| match item {
            FfiItem::Struct(structure)
                if structure
                    .annotations
                    .iter()
                    .any(|annotation| annotation.kind == AnnotationKind::Alias) =>
            {
                !defined.contains(&structure.name) && aliased.insert(structure.name.clone())
            }
            _ => true,
        })
        .collect()
}

/// Removes the GNU and MSVC spellings of attributes and calling conventions
/// (`__attribute__((...))`, `__declspec(...)`, `__stdcall`, ...), which a
/// declaration parser does not read.
fn without_decorations(text: &str) -> Result<String, Fault> {
    const GROUPED: [&str; 3] = ["__attribute__", "__declspec", "__asm__"];
    const BARE: [&str; 7] = [
        "__stdcall",
        "__cdecl",
        "__vectorcall",
        "__extension__",
        "__restrict",
        "__restrict__",
        "__inline",
    ];
    let tokens = tokenize(text, 1)?;
    let mut kept: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let word = token.kind == Kind::Ident;
        if word && GROUPED.contains(&token.text.as_str()) && tokens.get(index + 1).is_some_and(|t| t.is("(")) {
            index = after_group(&tokens, index + 1);
            continue;
        }
        if !(word && BARE.contains(&token.text.as_str())) {
            kept.push(token.clone());
        }
        index += 1;
    }
    Ok(render(&kept))
}

/// The index after the parenthesis group that opens at `open`.
fn after_group(tokens: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, token) in tokens.iter().enumerate().skip(open) {
        if token.is("(") {
            depth += 1;
        } else if token.is(")") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return offset + 1;
            }
        }
    }
    tokens.len()
}

const LONG: &str = "CLong";
const WCHAR: &str = "CWChar";

/// One `@FFI.Alias` for each C scalar whose width depends on the target
/// that the generated items use (section 26.2).
fn alias_declarations(items: &[FfiItem]) -> Vec<FfiItem> {
    [(LONG, "long"), (WCHAR, "wchar_t")]
        .into_iter()
        .filter(|(name, _)| items.iter().any(|item| uses(item, name)))
        .map(|(name, target)| {
            FfiItem::Struct(FfiStruct {
                annotations: vec![Annotation {
                    kind: AnnotationKind::Alias,
                    pairs: vec![("target".to_owned(), target.to_owned())],
                }],
                name: name.to_owned(),
                fields: Vec::new(),
            })
        })
        .collect()
}

fn uses(item: &FfiItem, name: &str) -> bool {
    match item {
        FfiItem::Function(function) => {
            function.result == name || function.parameters.iter().any(|(_, ty)| ty == name)
        }
        FfiItem::Struct(structure) => {
            structure.fields.iter().any(|field| field.ty == name)
                || structure
                    .annotations
                    .iter()
                    .any(|annotation| annotation.value("element") == Some(name))
        }
        FfiItem::Constant(constant) => constant.ty == name,
    }
}

/// The `.klffi` constant an object-like macro's value is: a string literal
/// as `CString`, an integer as `I32`, and anything else as no constant.
fn constant(define: &Define) -> Option<FfiConstant> {
    let value = define.value.trim();
    if value.is_empty() || unbindable_name(&define.name).is_some() {
        return None;
    }
    if value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && value.matches('"').count() == 2
    {
        return Some(FfiConstant {
            name: define.name.clone(),
            ty: "CString".to_owned(),
            value: value.to_owned(),
        });
    }
    let compact: String = value.split_whitespace().collect();
    let numeric = compact.trim_end_matches(['u', 'U', 'l', 'L']);
    let unsigned = numeric.strip_prefix('-').unwrap_or(numeric);
    integer_literal(unsigned)?;
    Some(FfiConstant {
        name: define.name.clone(),
        ty: "I32".to_owned(),
        value: numeric.to_owned(),
    })
}

pub(crate) fn parse_int(text: &str) -> Option<i64> {
    integer_literal(text)
}

fn prototype(
    statement: &str,
    library: &str,
    known: &mut Known,
) -> Result<Option<FfiFunction>, Remark> {
    let Some(open) = statement.find('(') else {
        return Ok(None);
    };
    let close = matching(statement, open).ok_or_else(|| "unbalanced parentheses".to_owned())?;
    if statement[close + 1..].trim_start().starts_with('(') {
        return Err(format!(
            "skipped function-pointer declaration: `{}`",
            brief(statement)
        ));
    }
    let head = statement[..open].trim();
    let (return_type, name) = split_name(head).ok_or_else(|| {
        format!(
            "skipped declaration without a clear name: `{}`",
            brief(statement)
        )
    })?;
    if !is_ident(&name) {
        return Ok(None);
    }
    if let Some(why) = unbindable_name(&name) {
        return Err(format!("skipped `{name}`: {why}"));
    }
    let result = known
        .map_type(&return_type, true)
        .ok_or_else(|| format!("skipped `{name}`: unmapped return type `{return_type}`"))?;
    let parameters = parse_params(&statement[open + 1..close], known)
        .map_err(|why| format!("skipped `{name}`: {why}"))?;
    let annotation = Annotation {
        kind: AnnotationKind::Extern,
        pairs: vec![
            ("library".to_owned(), library.to_owned()),
            ("symbol".to_owned(), name.clone()),
            ("abi".to_owned(), "c".to_owned()),
        ],
    };
    Ok(Some(FfiFunction {
        annotations: vec![annotation],
        name,
        parameters,
        result,
    }))
}

/// The `(name, type)` of each parameter of a prototype or callback. A C array
/// parameter decays to a pointer to its element, as C passes it.
pub(crate) fn parse_params(text: &str, known: &mut Known) -> Result<Vec<(String, FfiType)>, String> {
    let text = text.trim();
    if text.is_empty() || text == "void" {
        return Ok(Vec::new());
    }
    let mut parameters = Vec::new();
    for (index, raw) in text.split(',').enumerate() {
        let raw = raw.trim();
        if raw == "..." {
            return Err("variadic parameters need `@FFI.Variadic`".to_owned());
        }
        let (declarator, decays) = match raw.split_once('[') {
            Some((head, _)) => (head.trim(), true),
            None => (raw, false),
        };
        let (type_text, name) =
            split_name(declarator).unwrap_or_else(|| (declarator.to_owned(), String::new()));
        let type_text = if decays {
            format!("{type_text} *")
        } else {
            type_text
        };
        let ty = known
            .map_type(&type_text, true)
            .ok_or_else(|| format!("unmapped parameter type `{raw}`"))?;
        let name = if is_ident(&name) {
            parameter_name(&name, index)
        } else {
            format!("arg{index}")
        };
        parameters.push((name, ty));
    }
    Ok(parameters)
}

pub(crate) fn split_name(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let name_start = text.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    let name = text[name_start + 1..].to_owned();
    if name.is_empty() {
        return Some((text.to_owned(), String::new()));
    }
    let type_text = text[..=name_start].trim().to_owned();
    Some((type_text, name))
}

/// The `.klffi` spelling of a C scalar or pointer type, or `None`. A `char *`
/// is a `CString` where `text` is allowed, a raw pointer otherwise.
pub(crate) fn scalar_type(text: &str, text_allowed: bool) -> Option<FfiType> {
    let text = text.trim();
    let pointer = text.contains('*');
    let normalized = text.replace('*', " ");
    let words: Vec<&str> = normalized
        .split_whitespace()
        .filter(|w| {
            !matches!(
                *w,
                "const"
                    | "volatile"
                    | "restrict"
                    | "extern"
                    | "static"
                    | "inline"
                    | "register"
                    | "_Noreturn"
                    | "struct"
                    | "enum"
                    | "union"
            )
        })
        .collect();
    let base = words.join(" ");
    if pointer {
        if text_allowed && (base == "char" || base == "const char") {
            return Some("CString".to_owned());
        }
        return Some("RawPtr".to_owned());
    }
    Some(
        match base.as_str() {
            "void" => "Void",
            "_Bool" | "bool" => "Bool",
            "char" | "signed char" | "int8_t" | "__s8" => "I8",
            "unsigned char" | "uint8_t" | "__u8" => "U8",
            "short" | "short int" | "int16_t" | "__s16" => "I16",
            "unsigned short" | "unsigned short int" | "uint16_t" | "__u16" | "__le16"
            | "__be16" => "U16",
            "int" | "signed" | "signed int" | "int32_t" | "__s32" => "I32",
            "unsigned" | "unsigned int" | "uint32_t" | "__u32" | "__le32" | "__be32" => "U32",
            "long" | "long int" | "signed long" | "signed long int" => LONG,
            "long long" | "long long int" | "int64_t" | "__s64" => "I64",
            "intptr_t" | "ptrdiff_t" | "ssize_t" => "ISize",
            "wchar_t" => WCHAR,
            "unsigned long"
            | "unsigned long int"
            | "unsigned long long"
            | "unsigned long long int"
            | "uint64_t"
            | "__u64"
            | "__le64"
            | "__be64" => "U64",
            "size_t" | "uintptr_t" => "USize",
            "float" => "F32",
            "double" => "F64",
            _ => return None,
        }
        .to_owned(),
    )
}

pub(crate) fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn is_ident(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn brief(statement: &str) -> String {
    let flat: String = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() > 48 {
        format!("{}…", &flat[..48])
    } else {
        flat
    }
}
