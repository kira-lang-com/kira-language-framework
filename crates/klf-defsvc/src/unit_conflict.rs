//! What a conflict between two units says: the name, which units define it,
//! where each definition is written, and the first place the two differ. A
//! program that defines a name twice is told so; a conflict between two
//! lowerings of one definition is the compiler's fault, so the report carries
//! enough to find the unit that lowered the name wrongly.

use klf_kir::{Block, Function, Instruction, TypeDeclaration};
use klf_services::Session;
use klf_source::Span;
use std::fmt::Debug;

/// How long one side of a differing instruction may be in the report.
const SHOWN: usize = 160;

/// A unit that holds a definition: its name, and the definition.
pub(crate) struct Holder<'a> {
    pub unit: &'a str,
    pub function: &'a Function,
}

/// The name a diagnostic shows for an internal `name`. A generic instance is
/// written with its arguments (`textOf$Array<UN8>#Network` is `textOf<Array<UN8>>`),
/// a generic enum's instance number (`Gate_G1876825535242`) is dropped, and a
/// trailing `#<digits>` is removed.
pub(crate) fn source_name(name: &str) -> String {
    let name = strip_digit_suffix(name);
    if let Some((base, rest)) = name.split_once('$') {
        let arguments = rest.split_once('#').map_or(rest, |(arguments, _)| arguments);
        return format!("{base}<{arguments}>");
    }
    if let Some((base, number)) = name.rsplit_once("_G")
        && !base.is_empty()
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return base.to_owned();
    }
    name.to_owned()
}

fn strip_digit_suffix(name: &str) -> &str {
    match name.rsplit_once('#') {
        Some((base, suffix))
            if !base.is_empty() && !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => name,
    }
}

/// The unit a name is shown under: its name, or a stand-in when the unit has
/// none.
fn unit_label(unit: &str) -> String {
    if unit.is_empty() {
        "(unnamed unit)".to_owned()
    } else {
        format!("`{unit}`")
    }
}

/// The report of `name` defined differently by `first` and `second`.
pub(crate) fn function_conflict(
    session: &Session,
    name: &str,
    first: &Holder<'_>,
    second: &Holder<'_>,
) -> String {
    format!(
        "defines `{}` in more than one unit\n  \
         = note: unit {} defines it{}\n  \
         = note: unit {} defines it{}\n  \
         = note: they differ in {}\n  \
         = help: a program defines each name once; when both units lower the same \
         definition, a unit lowered from a stale cache entry, or an instance \
         specialized differently in one unit, gives this",
        source_name(name),
        unit_label(first.unit),
        place(session, first.function.span),
        unit_label(second.unit),
        place(session, second.function.span),
        difference(session, first.function, second.function),
    )
}

/// The report of type `name` declared differently by two units: where each
/// declaration is written and the first part of the two that differs.
pub(crate) fn type_conflict(
    session: &Session,
    first: &TypeDeclaration,
    second: &TypeDeclaration,
) -> String {
    format!(
        "declares `{}` differently in two units\n  \
         = note: one declaration is written{}\n  \
         = note: the other is written{}\n  \
         = note: they differ in {}\n  \
         = help: a program declares each type once; two units that lower the same generic \
         instance from different arguments, or a stale cache entry, give this",
        source_name(first.name.as_str()),
        place_of(session, first.span),
        place_of(session, second.span),
        type_difference(first, second),
    )
}

fn place_of(session: &Session, span: Span) -> String {
    let at = place(session, span);
    if at.is_empty() {
        " in a unit that is not registered".to_owned()
    } else {
        at
    }
}

/// The first part of two type declarations that is not the same.
fn type_difference(left: &TypeDeclaration, right: &TypeDeclaration) -> String {
    if left.kind != right.kind {
        return format!("their kind: {:?} against {:?}", left.kind, right.kind);
    }
    if left.layout != right.layout {
        return format!("their layout: {:?} against {:?}", left.layout, right.layout);
    }
    if left.generics != right.generics {
        return "their generic parameters".to_owned();
    }
    if left.fields != right.fields {
        return format!(
            "their fields: {} against {}",
            field_list(left),
            field_list(right)
        );
    }
    if left.variants != right.variants {
        return format!(
            "their cases: {} against {}",
            case_list(left),
            case_list(right)
        );
    }
    if left.open != right.open {
        return "whether the enum is `@Open`".to_owned();
    }
    if left.size != right.size || left.align != right.align {
        return "their size or alignment".to_owned();
    }
    "their policy".to_owned()
}

fn field_list(declaration: &TypeDeclaration) -> String {
    let names: Vec<String> = declaration
        .fields
        .iter()
        .map(|field| format!("`{}`", field.name.as_str()))
        .collect();
    if names.is_empty() { "none".to_owned() } else { names.join(", ") }
}

fn case_list(declaration: &TypeDeclaration) -> String {
    let names: Vec<String> = declaration
        .variants
        .iter()
        .map(|variant| match variant.payload.as_slice() {
            [] => format!("`{}`", variant.name.as_str()),
            payload => {
                let types: Vec<String> = payload.iter().map(ToString::to_string).collect();
                format!("`{}({})`", variant.name.as_str(), types.join(", "))
            }
        })
        .collect();
    if names.is_empty() { "none".to_owned() } else { names.join(", ") }
}

/// ` at path:line:column` for `span`, nothing when the session does not hold
/// its file.
fn place(session: &Session, span: Span) -> String {
    if !session.has_file(span.file) {
        return String::new();
    }
    let file = session.file(span.file);
    let at = file.line_column(span.start);
    format!(" at {}:{}:{}", file.name(), at.line, at.column)
}

/// The first part of the two definitions that is not the same.
fn difference(session: &Session, left: &Function, right: &Function) -> String {
    let signature: [(&str, bool); 9] = [
        ("linkage", left.linkage == right.linkage),
        ("ABI", left.abi == right.abi),
        ("unwinding", left.nounwind == right.nounwind),
        ("whether it is foreign", left.foreign == right.foreign),
        ("generic parameters", left.generics == right.generics),
        ("attributes", left.attributes == right.attributes),
        ("parameters", crate::drive::same_parameters(&left.parameters, &right.parameters)),
        ("result type", left.result == right.result),
        ("failure type", left.failure == right.failure),
    ];
    for (part, same) in signature {
        if !same {
            return format!("their {part}");
        }
    }
    if left.engine != right.engine {
        return "the engine they run on".to_owned();
    }
    if left.blocks.len() != right.blocks.len() {
        return format!(
            "their bodies: {} blocks against {}",
            left.blocks.len(),
            right.blocks.len()
        );
    }
    let mut anonymous = Vec::new();
    for (left, right) in left.blocks.iter().zip(&right.blocks) {
        if let Some(found) = block_difference(session, left, right, &mut anonymous) {
            return found;
        }
    }
    "nothing this report can name".to_owned()
}

fn block_difference(
    session: &Session,
    left: &Block,
    right: &Block,
    anonymous: &mut Vec<(String, String)>,
) -> Option<String> {
    if left.name != right.name {
        return Some(format!("block names: `{}` against `{}`", left.name, right.name));
    }
    if !crate::drive::same_parameters(&left.parameters, &right.parameters) {
        return Some(format!("the parameters of block `{}`", left.name));
    }
    if left.instructions.len() != right.instructions.len() {
        return Some(format!(
            "block `{}`: {} instructions against {}",
            left.name,
            left.instructions.len(),
            right.instructions.len()
        ));
    }
    for (index, (one, other)) in left.instructions.iter().zip(&right.instructions).enumerate() {
        if one.results == other.results
            && crate::drive::same_operation(&one.operation, &other.operation, anonymous)
        {
            continue;
        }
        return Some(format!(
            "block `{}`, instruction {}{}:\n      {}\n    against\n      {}",
            left.name,
            index + 1,
            instruction_place(session, one),
            shown(one),
            shown(other)
        ));
    }
    None
}

fn instruction_place(session: &Session, instruction: &Instruction) -> String {
    match &instruction.debug_location {
        Some(at) => format!(" (from {}:{}:{})", at.file, at.line, at.column),
        None => place(session, instruction.span).replacen(" at ", " (from ", 1)
            + if session.has_file(instruction.span.file) { ")" } else { "" },
    }
}

fn shown(instruction: &Instruction) -> String {
    let mut text = if instruction.results.is_empty() {
        debug_text(&instruction.operation)
    } else {
        let results: Vec<&str> = instruction.results.iter().map(klf_core::Name::as_str).collect();
        format!("{} = {}", results.join(", "), debug_text(&instruction.operation))
    };
    if text.chars().count() > SHOWN {
        text = text.chars().take(SHOWN).collect::<String>() + "…";
    }
    text
}

fn debug_text(value: &impl Debug) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_kir::{EnumVariant, Field, LayoutRule, Policy, TypeKind};
    use klf_core::FileId;

    fn span() -> Span {
        Span::new(FileId::from_u32(0), 0, 1)
    }

    fn enum_named(name: &str, cases: &[&str]) -> TypeDeclaration {
        let mut declaration = TypeDeclaration::empty(
            klf_core::Name::new(name),
            LayoutRule::Ridl,
            Policy::trivial(),
            span(),
        );
        declaration.kind = TypeKind::Enum;
        declaration.variants = cases
            .iter()
            .map(|case| EnumVariant {
                name: klf_core::Name::new(*case),
                payload: Vec::new(),
            })
            .collect();
        declaration
    }

    #[test]
    fn a_generic_enum_instance_shows_its_source_name() {
        assert_eq!(source_name("ApprovalGate_G1876825535242"), "ApprovalGate");
        assert_eq!(source_name("Gate_G7"), "Gate");
    }

    #[test]
    fn a_generic_instance_shows_its_arguments_and_no_unit() {
        assert_eq!(source_name("textOf$Array<UN8>#Network"), "textOf<Array<UN8>>");
        assert_eq!(source_name("textOf$Array<UN8>"), "textOf<Array<UN8>>");
    }

    #[test]
    fn a_trailing_digit_instance_number_is_dropped() {
        assert_eq!(source_name("helper#12"), "helper");
        assert_eq!(source_name("plain"), "plain");
        assert_eq!(source_name("_G12"), "_G12");
    }

    #[test]
    fn a_name_with_no_instance_marker_is_left_alone() {
        assert_eq!(source_name("ambientStateViewLogCount"), "ambientStateViewLogCount");
    }

    #[test]
    fn different_cases_name_both_case_lists() {
        let left = enum_named("Gate", &["Grant", "Refuse"]);
        let right = enum_named("Gate", &["Grant"]);
        assert_eq!(
            type_difference(&left, &right),
            "their cases: `Grant`, `Refuse` against `Grant`"
        );
    }

    #[test]
    fn different_kinds_are_named() {
        let left = enum_named("Gate", &["Grant"]);
        let mut right = enum_named("Gate", &["Grant"]);
        right.kind = TypeKind::Struct;
        assert_eq!(type_difference(&left, &right), "their kind: Enum against Struct");
    }

    #[test]
    fn different_fields_name_both_field_lists() {
        let mut left = enum_named("Point", &[]);
        left.kind = TypeKind::Struct;
        left.fields = vec![Field::new(klf_core::Name::new("x"), klf_kir::Type::Nominal(klf_core::Name::new("Number"), Vec::new()))];
        let right = TypeDeclaration { fields: Vec::new(), ..left.clone() };
        assert_eq!(type_difference(&left, &right), "their fields: `x` against none");
    }
}
