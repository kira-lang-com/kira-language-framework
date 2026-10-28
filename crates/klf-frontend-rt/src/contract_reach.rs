//! What a unit reads of the `.ridl` contracts it imports (sections 8.9, 9.3
//! and 20.1), for the query cache's red-green validation.
//!
//! A unit reaches a contract's declarations only by name: the names its own
//! sources spell, then the names the declarations it reaches spell, and so
//! on. A declaration none of those names reaches cannot change what the unit
//! lowers to, so the cache keeps, for each contract, the digest of only the
//! declarations the unit reaches. Adding a function to a library's contract
//! then leaves alone every unit that does not spell its name, which is what
//! lets an edit "re-run only the queries whose inputs changed" (section 9.3,
//! requirement 5) when the input is a contract.
//!
//! The contract is split by its layout, not parsed: a declaration starts at
//! a line in the first column and runs over the indented lines and closing
//! brackets after it, with the annotations and comments before it. Every line
//! of the text belongs to one declaration, so nothing in it goes unread. A
//! declaration is reached through the name it declares; one whose name the
//! first line does not plainly give is reached through every name on that
//! line, and one with none, such as an import, is always reached. A name
//! reaches a declaration whose name it is or is a `_`-separated part of, so
//! the functions a derive names after a type, such as `eq_Type`, are reached
//! with the type.
//!
//! The functions in a type's or an extension's body are declarations of
//! their own, reached through their names: a unit that names a type but
//! never calls one of its methods does not read that method's signature,
//! nor the types only that signature spells. The functions the definition
//! calls without a source naming them stay with their type.

use klf_ridl::sha256::sha256;
use std::collections::{BTreeSet, HashMap};

/// The words that open a declaration without naming it.
const OPENERS: [&str; 22] = [
    "public",
    "struct",
    "enum",
    "class",
    "trait",
    "function",
    "let",
    "var",
    "type",
    "construct",
    "namespace",
    "distinct",
    "macro",
    "comptime",
    "async",
    "indirect",
    "final",
    "open",
    "mutating",
    "static",
    "init",
    "case",
];

/// The words before `function` in a member, or before what a declaration
/// is.
const MODIFIERS: [&str; 10] = [
    "public", "static", "mutating", "final", "open", "async", "indirect", "override", "consuming",
    "borrowing",
];

/// How far a type's or an extension's members are indented in a contract.
const MEMBER_INDENT: usize = 4;

/// The functions the Kira definition calls by name without a source
/// spelling it, such as a raw enum's `rawValue`: kept with their type, which
/// reaches them.
const IMPLICIT: [&str; 26] = [
    "rawValue", "fromRawValue", "projectedValue", "wrappedValue", "init", "drop", "destroy",
    "copy", "hash", "equal", "order", "next", "description", "get", "set", "absorb", "union",
    "contains", "append", "close", "send", "receive", "teardown", "bootstrap", "trace", "default",
];

/// The types every program can spell without a source naming them, such as
/// a literal's.
const BUILTIN: [&str; 26] = [
    "Number", "UNumber", "Int", "Float", "Bool", "String", "Void", "Any", "RawPtr", "Optional",
    "Result", "Array", "I8", "I16", "I32", "U8", "U16", "U32", "U64", "F32", "N8", "N16", "N32",
    "UN8", "UN16", "UN32",
];

/// One declaration of a contract.
struct Declaration<'text> {
    /// The names that reach it; none for one always reached.
    names: Vec<&'text str>,
    lines: Vec<&'text str>,
}

/// The names `texts` spell, with the builtin types: where a unit's reach
/// into its contracts starts.
pub(crate) fn seed<'text>(texts: impl IntoIterator<Item = &'text str>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BUILTIN.iter().map(|name| (*name).to_owned()).collect();
    for text in texts {
        for name in identifiers(text) {
            if !names.contains(name) {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

/// The digest of what `seed` reaches of each of `contracts`, in order. The
/// contracts are reached together, as a declaration of one names the types
/// of another.
pub(crate) fn reached<'text>(seed: &BTreeSet<String>, contracts: &[&'text str]) -> Vec<[u8; 32]> {
    let declarations: Vec<Vec<Declaration<'text>>> =
        contracts.iter().map(|text| declarations(text)).collect();
    let mut through: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    let mut taken: Vec<Vec<bool>> = declarations
        .iter()
        .map(|list| vec![false; list.len()])
        .collect();
    let mut pending: Vec<(usize, usize)> = Vec::new();
    for (contract, list) in declarations.iter().enumerate() {
        for (index, declaration) in list.iter().enumerate() {
            if declaration.names.is_empty() {
                pending.push((contract, index));
                continue;
            }
            for name in &declaration.names {
                for part in std::iter::once(*name).chain(name.split('_')) {
                    if !part.is_empty() {
                        through.entry(part).or_default().push((contract, index));
                    }
                }
            }
        }
    }
    let mut known: BTreeSet<&str> = BTreeSet::new();
    for name in seed {
        learn(&through, name, &mut pending, &mut known);
        for part in name.split('_') {
            learn(&through, part, &mut pending, &mut known);
        }
    }
    while let Some((contract, index)) = pending.pop() {
        if std::mem::replace(&mut taken[contract][index], true) {
            continue;
        }
        for line in &declarations[contract][index].lines {
            for name in identifiers(line) {
                learn(&through, name, &mut pending, &mut known);
            }
        }
    }
    declarations
        .iter()
        .zip(&taken)
        .map(|(list, taken)| {
            let mut text = String::new();
            for (declaration, taken) in list.iter().zip(taken) {
                if *taken {
                    for line in &declaration.lines {
                        text.push_str(line);
                        text.push('\n');
                    }
                    text.push('\0');
                }
            }
            sha256(text.as_bytes())
        })
        .collect()
}

/// Takes `name` as reached: every declaration it reaches is pending, once.
/// A name the declarations do not spell reaches nothing; only theirs are
/// kept, so `known` borrows from the contracts.
fn learn<'text>(
    through: &HashMap<&'text str, Vec<(usize, usize)>>,
    name: &str,
    pending: &mut Vec<(usize, usize)>,
    known: &mut BTreeSet<&'text str>,
) {
    if let Some((key, found)) = through.get_key_value(name)
        && known.insert(key)
    {
        pending.extend(found.iter().copied());
    }
}

fn declarations(text: &str) -> Vec<Declaration<'_>> {
    let mut list: Vec<Declaration<'_>> = Vec::new();
    // Annotations and comments, kept for the declaration they come before.
    let mut leading: Vec<&str> = Vec::new();
    // The declaration whose lines the indented ones after it continue.
    let mut top: Option<usize> = None;
    // Whether `top` is a type or an extension, whose functions are their own
    // declarations, and the one of them now open.
    let mut members = false;
    let mut member: Option<usize> = None;
    // A member's annotations and comments, kept for it.
    let mut inner: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let first = line.chars().next().unwrap_or(' ');
        if first.is_whitespace() || matches!(first, '}' | ')' | ']') {
            match top {
                Some(owner) if leading.is_empty() => {
                    let body = line.trim_start();
                    let outer = first.is_whitespace()
                        && line.len() - body.len() == MEMBER_INDENT
                        && !body.starts_with(['}', ')', ']']);
                    if !members {
                        list[owner].lines.push(line);
                    } else if outer && (body.starts_with('@') || body.starts_with("//")) {
                        inner.push(line);
                    } else if let Some(name) = outer.then(|| member_name(body)).flatten() {
                        let mut lines = std::mem::take(&mut inner);
                        lines.push(line);
                        list.push(Declaration {
                            names: vec![name],
                            lines,
                        });
                        member = Some(list.len() - 1);
                    } else if let (Some(open), false) = (member, outer || !first.is_whitespace()) {
                        list[open].lines.push(line);
                    } else {
                        member = None;
                        list[owner].lines.append(&mut inner);
                        list[owner].lines.push(line);
                    }
                }
                _ => {
                    leading.push(line);
                    list.push(Declaration {
                        names: Vec::new(),
                        lines: std::mem::take(&mut leading),
                    });
                    top = Some(list.len() - 1);
                    members = false;
                    member = None;
                }
            }
            continue;
        }
        if first == '@' || line.starts_with("//") {
            leading.push(line);
            continue;
        }
        if let Some(owner) = top {
            list[owner].lines.append(&mut inner);
        }
        let mut lines = std::mem::take(&mut leading);
        lines.push(line);
        list.push(Declaration {
            names: names_of(line),
            lines,
        });
        top = Some(list.len() - 1);
        members = has_members(line);
        member = None;
    }
    if let Some(owner) = top {
        list[owner].lines.append(&mut inner);
    }
    if !leading.is_empty() {
        list.push(Declaration {
            names: Vec::new(),
            lines: leading,
        });
    }
    list
}

/// Whether `line` opens a type or an extension whose body lists functions.
fn has_members(line: &str) -> bool {
    line.trim_end().ends_with('{')
        && identifiers(line)
            .find(|word| !MODIFIERS.contains(word))
            .is_some_and(|word| matches!(word, "struct" | "class" | "extend"))
}

/// The name of the function a member line declares, when it is reached
/// through that name alone: not an operator, nor one the definition calls
/// without a source spelling it.
fn member_name(body: &str) -> Option<&str> {
    let mut rest = body;
    loop {
        let word: &str = identifiers(rest).next()?;
        let at = rest.find(word)?;
        if !rest[..at].trim().is_empty() {
            return None;
        }
        rest = &rest[at + word.len()..];
        if word == "function" {
            break;
        }
        if !MODIFIERS.contains(&word) {
            return None;
        }
    }
    let rest = rest.strip_prefix(' ')?;
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..end];
    let starts = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    (starts && !IMPLICIT.contains(&name)).then_some(name)
}

/// The names that reach the declaration `line` opens.
fn names_of(line: &str) -> Vec<&str> {
    let words: Vec<&str> = identifiers(line).collect();
    match words.first() {
        Some(&"import") | None => Vec::new(),
        // The type an extension extends, every name it is spelled with.
        Some(&"extend") => {
            let target = line
                .split([':', '{'])
                .next()
                .unwrap_or(line);
            identifiers(target).skip(1).collect()
        }
        Some(word) if OPENERS.contains(word) => words
            .iter()
            .find(|word| !OPENERS.contains(word))
            .map_or_else(Vec::new, |name| vec![*name]),
        // A shape this reading does not know, such as a family's member:
        // whatever it names.
        Some(_) => words,
    }
}

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::{reached, seed};

    const LIBRARY: &str = "import Foundation\n\nstruct Point {\n    let x: Number\n    let y: Number\n}\n\nfunction origin() -> Point\n\nfunction unused() -> Number\n\nextend Point {\n    function length(borrow self) -> Number\n}\n";

    #[test]
    fn a_declaration_no_name_reaches_leaves_the_digest_alone() {
        let names = seed(["let p = origin()"]);
        let before = reached(&names, &[LIBRARY]);
        let added = format!("{LIBRARY}\nfunction another() -> Number\n");
        assert_eq!(reached(&names, &[&added]), before);
        let changed = LIBRARY.replace("let y: Number", "let y: Number\n    let z: Number");
        assert_ne!(reached(&names, &[&changed]), before);
        let spelled = format!("{LIBRARY}\nfunction origin(scale: Number) -> Point\n");
        assert_ne!(reached(&names, &[&spelled]), before);
    }

    #[test]
    fn a_name_reaches_across_contracts_and_derived_names() {
        let names = seed(["let p = origin()"]);
        let other = "struct Unrelated {\n}\n\nfunction eq_Point(a: Point, b: Point) -> Bool\n";
        let before = reached(&names, &[LIBRARY, other]);
        let edited = other.replace("b: Point) -> Bool", "b: Point, c: Number) -> Bool");
        assert_ne!(reached(&names, &[LIBRARY, &edited]), before);
        let unrelated = other.replace("struct Unrelated {\n}", "struct Unrelated {\n    let a: Number\n}");
        assert_eq!(reached(&names, &[LIBRARY, &unrelated]), before);
    }

    #[test]
    fn a_method_never_called_leaves_its_types_alone() {
        let library = "struct Application {\n    let name: String\n}\n\nenum Request {\n    Open(String)\n}\n\nextend Application {\n    function path(borrow self) -> String\n    // Asks for it.\n    function openRequest(borrow self) -> Request\n    function rawValue(borrow self) -> Number\n}\n";
        let names = seed(["let a: Application = load()\nlet p = a.path()"]);
        let before = reached(&names, &[library]);
        let added = library.replace("    Open(String)", "    Open(String)\n    Switch(String)");
        assert_eq!(reached(&names, &[&added]), before);
        let field = library.replace("    let name: String", "    let name: String\n    let size: Number");
        assert_ne!(reached(&names, &[&field]), before);
        let implicit = library.replace("rawValue(borrow self) -> Number", "rawValue(borrow self) -> U32");
        assert_ne!(reached(&names, &[&implicit]), before);
        let calls = seed(["let a: Application = load()\nlet r = a.openRequest()"]);
        assert_ne!(reached(&calls, &[&added]), reached(&calls, &[library]));
    }
}
