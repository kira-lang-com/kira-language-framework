//! The declarations of a C header other than function prototypes: structs,
//! enums and typedefs, for `klf ffi gen` (section 26.5).
//!
//! A struct becomes an `@FFI.Struct { layout: c }` with its fields in order, a
//! fixed array field an `@FFI.Array` type of its own, and a bit field a field
//! with `@Bits`. An enum's members become integer constants. A typedef names
//! what it stands for, so a later declaration that writes the typedef's name
//! maps to the same `.klffi` type. What has no `.klffi` form (a union, an
//! anonymous nested struct, an array whose length is not a number) is left
//! out with a remark, and so is any struct that uses it by value.

use crate::cgen::{
    Remark, brief, is_ident, matching, parse_int, parse_params, scalar_type, split_name,
};
use crate::model::{
    Annotation, AnnotationKind, FfiConstant, FfiField, FfiItem, FfiStruct, FfiType,
};
use crate::names::{field_name, unbindable_name};
use std::collections::{BTreeMap, BTreeSet};

/// The names a header has declared so far, which later declarations use.
#[derive(Debug, Default)]
pub(crate) struct Known {
    /// Structs with a `.klffi` declaration, by the name `.klffi` gives them.
    structs: BTreeSet<String>,
    /// A typedef's or a struct tag's name, and the C type it stands for.
    aliases: BTreeMap<String, String>,
    /// Integer constants from enums, which may size an array.
    constants: BTreeMap<String, i64>,
    /// The `@FFI.Array` types already declared.
    arrays: BTreeSet<String>,
    /// The `@FFI.Pointer` types already declared, by the name `.klffi` gives.
    pointers: BTreeSet<String>,
    /// The opaque struct tags whose `@FFI.Alias` is declared.
    opaques: BTreeSet<String>,
    /// Declarations that mapping a type made, written before the declaration
    /// that uses them.
    pub(crate) pending: Vec<FfiItem>,
    /// The `@FFI.Pointer` names another header of the package declares, which
    /// are not declared here.
    pub(crate) withheld: BTreeSet<String>,
}

impl Known {
    /// The `.klffi` spelling of the C type `text`, through the typedefs and
    /// structs declared so far; `None` when it has none. A `char *` is a
    /// `CString` where `text_allowed`.
    pub(crate) fn map_type(&mut self, text: &str, text_allowed: bool) -> Option<FfiType> {
        self.resolve(text, text_allowed, 0)
    }

    fn resolve(&mut self, text: &str, text_allowed: bool, depth: usize) -> Option<FfiType> {
        if depth > 16 {
            return None;
        }
        let pointer = text.contains('*');
        if let Some(base) = base_name(text)
            && let Some(aliased) = self.aliases.get(&base).cloned()
        {
            let stars = if pointer { " *" } else { "" };
            return self.resolve(&format!("{aliased}{stars}"), text_allowed, depth + 1);
        }
        if pointer {
            return self.pointer(text, text_allowed, depth);
        }
        if let Some(mapped) = scalar_type(text, text_allowed) {
            return Some(mapped);
        }
        let base = base_name(text)?;
        if self.structs.contains(&base) {
            return Some(base);
        }
        if is_struct_tag(text) {
            self.declare_opaque(&base);
            return Some(base);
        }
        None
    }

    /// The `@FFI.Alias` of an opaque struct tag, declared once.
    fn declare_opaque(&mut self, tag: &str) {
        if self.opaques.insert(tag.to_owned()) {
            self.pending.push(opaque_alias(tag));
        }
    }

    /// A pointer type. `void *` is `RawPtr` and `char *` a `CString` where
    /// `text_allowed`. Any other pointer is `<pointee>_ptr`, declared once with
    /// `@FFI.Pointer` to its pointee; a pointer to a pointer names the inner
    /// pointer's `.klffi` type, as `VkDevice_T_ptr_ptr` does.
    fn pointer(&mut self, text: &str, text_allowed: bool, depth: usize) -> Option<FfiType> {
        let (pointee, target) = if text.matches('*').count() == 1 {
            let spelled = spelled(text)?;
            if is_struct_tag(text) && !self.structs.contains(&spelled) {
                self.declare_opaque(&spelled);
            }
            match spelled.as_str() {
                "void" => return Some("RawPtr".to_owned()),
                "char" => {
                    return Some(if text_allowed { "CString" } else { "RawPtr" }.to_owned());
                }
                _ => {}
            }
            let target = scalar_type(&spelled, false).unwrap_or_else(|| spelled.clone());
            (spelled.replace(' ', "_"), target)
        } else {
            let mut inner = text.to_owned();
            let last = inner.rfind('*')?;
            inner.remove(last);
            let pointee = self.resolve(&inner, false, depth + 1)?;
            (pointee.clone(), pointee)
        };
        let name = format!("{pointee}_ptr");
        // A name another header of the package declares is used but not declared
        // again: it still resolves to that header's declaration.
        let withheld = self.withheld.contains(&name);
        if self.pointers.insert(name.clone()) && !withheld {
            self.pending.push(FfiItem::Struct(FfiStruct {
                annotations: vec![Annotation {
                    kind: AnnotationKind::Pointer,
                    pairs: vec![
                        ("target".to_owned(), target),
                        ("ownership".to_owned(), "borrowed".to_owned()),
                    ],
                }],
                name: name.clone(),
                fields: Vec::new(),
            }));
        }
        Some(name)
    }
}

/// Whether `text` names a struct by its tag, with the `struct` keyword.
fn is_struct_tag(text: &str) -> bool {
    text.split_whitespace().any(|word| word == "struct")
}

/// The words a type is spelled with, without its qualifiers, `struct` and
/// `enum` keywords and stars, or `None` when no word is left.
fn spelled(text: &str) -> Option<String> {
    let normalized = text.replace('*', " ");
    let words: Vec<&str> = normalized
        .split_whitespace()
        .filter(|word| {
            !matches!(
                *word,
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
    (!words.is_empty()).then(|| words.join(" "))
}

/// The `@FFI.Alias` of an opaque struct tag, which has no body.
pub(crate) fn opaque_alias(tag: &str) -> FfiItem {
    FfiItem::Struct(FfiStruct {
        annotations: vec![Annotation {
            kind: AnnotationKind::Alias,
            pairs: vec![("target".to_owned(), tag.to_owned())],
        }],
        name: tag.to_owned(),
        fields: Vec::new(),
    })
}

/// The one name a type is written with once its qualifiers and any `struct`
/// are gone, or `None` when it is more than one word.
fn base_name(text: &str) -> Option<String> {
    let normalized = text.replace('*', " ");
    let words: Vec<&str> = normalized
        .split_whitespace()
        .filter(|w| {
            !matches!(
                *w,
                "const" | "volatile" | "struct" | "enum" | "extern" | "static"
            )
        })
        .collect();
    match words.as_slice() {
        [word] if is_ident(word) => Some((*word).to_owned()),
        _ => None,
    }
}

/// The header's top-level declarations, each without its `;` and with its
/// whitespace made single spaces: preprocessor lines left out,
/// `extern "C" { ... }` opened up, and a function's body dropped together
/// with its definition.
pub(crate) fn top_level(text: &str) -> Vec<String> {
    let mut code = String::with_capacity(text.len());
    let mut continued = false;
    for line in text.lines() {
        let directive = continued || line.trim_start().starts_with('#');
        continued = directive && line.trim_end().ends_with('\\');
        if !directive {
            code.push_str(line);
            code.push('\n');
        }
    }
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut opened = 0usize;
    for c in code.chars() {
        match c {
            '{' if depth == 0 && current.trim() == "extern \"C\"" => {
                opened += 1;
                current.clear();
            }
            '}' if depth == 0 && opened > 0 => {
                opened -= 1;
                current.clear();
            }
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(c);
                let head = current.split('{').next().unwrap_or("").trim_end();
                if depth == 0 && head.ends_with(')') {
                    current.clear();
                }
            }
            ';' if depth == 0 => {
                let statement = current.split_whitespace().collect::<Vec<_>>().join(" ");
                if !statement.is_empty() {
                    statements.push(statement);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    statements
}

/// What a `typedef`, `struct`, `union` or `enum` statement declares, or
/// `None` when the statement is none of them.
pub(crate) fn declaration(
    statement: &str,
    known: &mut Known,
) -> Option<Result<Vec<FfiItem>, Remark>> {
    if let Some(rest) = statement.strip_prefix("typedef ") {
        return Some(typedef(rest.trim(), known));
    }
    let keyword = ["struct ", "union ", "enum "]
        .into_iter()
        .find(|keyword| statement.starts_with(keyword))?;
    // `struct tag *function(...)` is a prototype that returns a pointer.
    if !statement.contains('{') && statement.contains('(') {
        return None;
    }
    let tagged = Tagged::read(statement)?;
    if !tagged.rest.is_empty() {
        return Some(Err(format!(
            "skipped a variable declaration: `{}`",
            brief(statement)
        )));
    }
    let name = tagged.tag.clone();
    Some(define(keyword.trim(), &tagged, name, known))
}

/// A `struct`, `union` or `enum` written with its tag, body and what follows.
struct Tagged {
    tag: Option<String>,
    body: Option<String>,
    rest: String,
}

impl Tagged {
    fn read(text: &str) -> Option<Self> {
        let after = text.split_once(' ')?.1.trim();
        let (head, body, rest) = if let Some(open) = after.find('{') {
            let close = matching_brace(after, open)?;
            (
                after[..open].trim(),
                Some(after[open + 1..close].to_owned()),
                after[close + 1..].trim(),
            )
        } else {
            let (tag, rest) = after.split_once(' ').unwrap_or((after, ""));
            (tag, None, rest.trim())
        };
        let tag = (!head.is_empty() && is_ident(head)).then(|| head.to_owned());
        Some(Self {
            tag,
            body,
            rest: rest.to_owned(),
        })
    }
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (offset, byte) in text.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
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

/// `text` with each `*` set off by spaces, so `struct tag* name` reads as
/// `struct tag * name` whatever the header's spacing was.
fn spaced_stars(text: &str) -> String {
    text.replace('*', " * ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn typedef(rest: &str, known: &mut Known) -> Result<Vec<FfiItem>, Remark> {
    let skipped = || format!("skipped a typedef: `{}`", brief(rest));
    // `typedef int (*callback)(void *)`: a function pointer, a `@FFI.Callback`.
    // A calling convention macro expanded to nothing leaves a space: `( *name)`.
    let pointer_open = rest
        .match_indices('(')
        .map(|(index, _)| index)
        .find(|&index| rest[index + 1..].trim_start().starts_with('*'));
    if let Some(open) = pointer_open {
        let close = matching(rest, open).ok_or_else(skipped)?;
        let name = rest[open + 1..close].trim().trim_start_matches('*').trim();
        if !is_ident(name) {
            return Err(skipped());
        }
        let tail = rest[close + 1..].trim_start();
        if !tail.starts_with('(') {
            return Err(skipped());
        }
        let params_open = rest.len() - tail.len();
        let params_close = matching(rest, params_open).ok_or_else(skipped)?;
        let result_text = rest[..open].trim();
        let result = known
            .map_type(result_text, true)
            .ok_or_else(|| format!("skipped callback `{name}`: unmapped result `{result_text}`"))?;
        let parameters = parse_params(&rest[params_open + 1..params_close], known)
            .map_err(|why| format!("skipped callback `{name}`: {why}"))?;
        let types: Vec<String> = parameters.into_iter().map(|(_, ty)| ty).collect();
        known.structs.insert(name.to_owned());
        return Ok(vec![FfiItem::Struct(FfiStruct {
            annotations: vec![Annotation {
                kind: AnnotationKind::Callback,
                pairs: vec![
                    ("abi".to_owned(), "c".to_owned()),
                    ("params".to_owned(), format!("[{}]", types.join(", "))),
                    ("result".to_owned(), result),
                ],
            }],
            name: name.to_owned(),
            fields: Vec::new(),
        })]);
    }
    let keyword = ["struct ", "union ", "enum "]
        .into_iter()
        .find(|keyword| rest.starts_with(keyword));
    if let Some(keyword) = keyword {
        let tagged = Tagged::read(&spaced_stars(rest)).ok_or_else(skipped)?;
        let alias = tagged.rest.trim();
        let pointer = alias.starts_with('*');
        let alias = alias.trim_start_matches('*').trim();
        if !is_ident(alias) {
            return Err(skipped());
        }
        if tagged.body.is_none() || pointer {
            let target = tagged.tag.clone().unwrap_or_default();
            let stars = if pointer { " *" } else { "" };
            known.aliases.insert(
                alias.to_owned(),
                format!("{} {target}{stars}", keyword.trim()),
            );
            let opaque = keyword.trim() == "struct" && tagged.body.is_none() && !target.is_empty();
            return Ok(opaque.then(|| opaque_alias(&target)).into_iter().collect());
        }
        // The typedef's name is the struct's name; its tag stands for it.
        if let Some(tag) = &tagged.tag
            && tag != alias
        {
            known.aliases.insert(tag.clone(), alias.to_owned());
        }
        return define(keyword.trim(), &tagged, Some(alias.to_owned()), known);
    }
    let (type_text, name) = split_name(rest).ok_or_else(skipped)?;
    if name.is_empty() || !is_ident(&name) {
        return Err(skipped());
    }
    known.aliases.insert(name, type_text);
    Ok(Vec::new())
}

/// The items a tagged declaration with a body makes, under `name`.
fn define(
    keyword: &str,
    tagged: &Tagged,
    name: Option<String>,
    known: &mut Known,
) -> Result<Vec<FfiItem>, Remark> {
    let Some(body) = &tagged.body else {
        // A forward declaration: the struct is opaque until defined.
        return Ok(Vec::new());
    };
    match keyword {
        "enum" => {
            if let Some(name) = &name {
                known.aliases.insert(name.clone(), "int".to_owned());
            }
            Ok(enumerators(body, known))
        }
        "union" => {
            let shown = name.unwrap_or_else(|| "an anonymous union".to_owned());
            Err(format!(
                "skipped union `{shown}`: unions have no generated form"
            ))
        }
        _ => {
            let name = name.ok_or_else(|| "skipped an anonymous struct".to_owned())?;
            if let Some(why) = unbindable_name(&name) {
                return Err(format!("skipped struct `{name}`: {why}"));
            }
            let mut items = Vec::new();
            // Known before its fields, so a pointer to the struct inside it is its own type.
            known.structs.insert(name.clone());
            let fields = match fields(&name, body, known, &mut items) {
                Ok(fields) => fields,
                Err(remark) => {
                    known.structs.remove(&name);
                    return Err(remark);
                }
            };
            items.push(FfiItem::Struct(FfiStruct {
                annotations: vec![Annotation {
                    kind: AnnotationKind::Struct,
                    pairs: vec![("layout".to_owned(), "c".to_owned())],
                }],
                name,
                fields,
            }));
            Ok(items)
        }
    }
}

fn enumerators(body: &str, known: &mut Known) -> Vec<FfiItem> {
    let mut items = Vec::new();
    let mut next = 0i64;
    for member in body.split(',') {
        let member = member.trim();
        if member.is_empty() {
            continue;
        }
        let (name, value) = match member.split_once('=') {
            Some((name, value)) => {
                let value = value.trim();
                let numeric = value.trim_end_matches(['u', 'U', 'l', 'L']);
                let Some(parsed) =
                    parse_int(numeric).or_else(|| known.constants.get(value).copied())
                else {
                    break;
                };
                (name.trim(), parsed)
            }
            None => (member, next),
        };
        if !is_ident(name) {
            break;
        }
        known.constants.insert(name.to_owned(), value);
        // A member named for a Kira keyword keeps its value for the members after
        // it, but is not declared: no constant can carry that name.
        if unbindable_name(name).is_none() {
            items.push(FfiItem::Constant(FfiConstant {
                name: name.to_owned(),
                ty: "I32".to_owned(),
                value: value.to_string(),
            }));
        }
        next = value + 1;
    }
    items
}

/// A struct's fields, in order. A fixed array's type is declared into
/// `items` first.
fn fields(
    name: &str,
    body: &str,
    known: &mut Known,
    items: &mut Vec<FfiItem>,
) -> Result<Vec<FfiField>, Remark> {
    let mut fields = Vec::new();
    for declaration in body.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        if declaration.contains('{') || declaration.contains('(') {
            return Err(format!(
                "skipped struct `{name}`: a nested declaration or function pointer in `{}`",
                brief(declaration)
            ));
        }
        let mut parts = declaration.split(',');
        let first = parts.next().unwrap_or("").trim();
        let named = first.split([':', '[']).next().unwrap_or("");
        let (base, _) = split_name(named)
            .ok_or_else(|| format!("skipped struct `{name}`: cannot read `{declaration}`"))?;
        let base = base.trim_end_matches(['*', ' ']).to_owned();
        let declarators = std::iter::once(first[base.len()..].trim().to_owned())
            .chain(parts.map(|part| part.trim().to_owned()));
        for declarator in declarators {
            let mut entry = field(name, &base, &declarator, known, items)?;
            let taken: Vec<String> = fields.iter().map(|f: &FfiField| f.name.clone()).collect();
            entry.name = field_name(&entry.name, &taken)
                .ok_or_else(|| format!("skipped struct `{name}`: a field with no name"))?;
            fields.push(entry);
        }
    }
    Ok(fields)
}

/// One field: `declarator` is what follows the base type, such as `*next`,
/// `bytes[16]` or `flag : 1`.
fn field(
    structure: &str,
    base: &str,
    declarator: &str,
    known: &mut Known,
    items: &mut Vec<FfiItem>,
) -> Result<FfiField, Remark> {
    let (declarator, bits) = match declarator.split_once(':') {
        Some((declarator, width)) => {
            let width = width.trim().parse::<u32>().map_err(|_| {
                format!(
                    "skipped struct `{structure}`: bit field width `{}`",
                    width.trim()
                )
            })?;
            (declarator.trim(), Some(width))
        }
        None => (declarator, None),
    };
    let pointer = declarator.starts_with('*');
    let declarator = declarator.trim_start_matches(['*', ' ']);
    let (field_name, length) = match declarator.split_once('[') {
        Some((field_name, rest)) => {
            let count = rest.trim_end_matches(']').trim();
            let length = parse_int(count)
                .or_else(|| known.constants.get(count).copied())
                .ok_or_else(|| format!("skipped struct `{structure}`: array length `{count}`"))?;
            (field_name.trim(), Some(length))
        }
        None => (declarator.trim(), None),
    };
    if !is_ident(field_name) {
        return Err(format!(
            "skipped struct `{structure}`: field `{declarator}`"
        ));
    }
    let written = if pointer {
        format!("{base} *")
    } else {
        base.to_owned()
    };
    let element = known
        .map_type(&written, false)
        .filter(|mapped| mapped != "Void")
        .ok_or_else(|| {
            format!("skipped struct `{structure}`: field `{field_name}` has type `{written}`")
        })?;
    let ty = match length {
        Some(count) => {
            let array = format!("{element}_array_{count}");
            if known.arrays.insert(array.clone()) {
                items.push(FfiItem::Struct(FfiStruct {
                    annotations: vec![Annotation {
                        kind: AnnotationKind::Array,
                        pairs: vec![
                            ("element".to_owned(), element),
                            ("count".to_owned(), count.to_string()),
                        ],
                    }],
                    name: array.clone(),
                    fields: Vec::new(),
                }));
            }
            array
        }
        None => element,
    };
    Ok(FfiField {
        name: field_name.to_owned(),
        ty,
        bits,
    })
}
