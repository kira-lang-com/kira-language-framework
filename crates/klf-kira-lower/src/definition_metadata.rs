use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{
    CMember, ConstructDecl, DeclKind, Expr, ExprKind, FieldInit, File, MemberKind, TypeKind,
};
use klf_services::{
    DefaultConvention, EntryConvention, Exclusivity, LanguageDescriptor, MemoryModel, Session,
};
use klf_source::Span;
use std::collections::HashMap;

use crate::definition_entrypoints::extract_entrypoints;

type MetadataFields<'a> = HashMap<&'a str, &'a Expr>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefinitionEntrypoints {
    pub units: Span,
    pub parse: Span,
    pub elaborate: Span,
    pub lower: Span,
    pub compat_driver: Option<Span>,
    pub run_program: Option<Span>,
    /// A shader language's out (`KLF-KSL.md` section 5.8).
    pub emit: Option<Span>,
    /// The formatter (`KLF-FORMAT.md` section 3).
    pub format: Option<Span>,
    /// Where a file belongs in the editor (`KLF-EDITOR.md` section 5.3).
    pub project: Option<Span>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageDefinitionMetadata {
    pub descriptor: LanguageDescriptor,
    pub entrypoints: DefinitionEntrypoints,
}

/// Extracts the required section-7 metadata from one KLF Kira `Language`
/// construct. Phase members are intentionally not executed here; this is the
/// typed metadata boundary used before an executable definition image exists.
#[must_use]
pub fn extract_language_descriptor(
    session: &mut Session,
    file: &File,
) -> Option<LanguageDescriptor> {
    let construct = language_construct(session, file)?;
    let shader = is_shader_construct(session, construct)?;
    let fields = metadata_fields(session, construct)?;

    let name = required_string(session, &fields, "name")?;
    let driver = required_string(session, &fields, "driver")?;
    let version = required_string(session, &fields, "version")?;
    let extensions = required_strings(session, &fields, "extensions")?;
    let family = if shader {
        Some(crate::definition_shader::shader_family(session, &fields)?)
    } else {
        None
    };
    let (memory, exclusivity, convention, std_root) = language_model(session, &fields, shader)?;
    let entry = optional_entry(session, &fields, "entry")?;
    let file_modes = crate::definition_file_modes::optional_file_modes(
        session,
        fields.get("fileModes").copied(),
    )?;
    let mut flags = crate::definition_flags::optional_flags(session, fields.get("flags").copied())?;
    let members: Vec<&klf_kira_syntax::ast::Member> = construct
        .members
        .iter()
        .filter_map(|member| match member {
            CMember::Member(member) => Some(member.as_ref()),
            _ => None,
        })
        .collect();
    let (cli, options) = crate::cli::spec::language_command(session, file, &members, &driver)?;
    flags.extend(options);
    let diagnostics = crate::definition_diagnostics::optional_diagnostics(
        session,
        fields.get("diagnostics").copied(),
    )?;
    let editor = crate::definition_editor::optional_editor(session, fields.get("editor").copied())?;

    Some(
        LanguageDescriptor::new(
            name,
            driver,
            version,
            extensions,
            memory,
            exclusivity,
            convention,
            std_root,
        )
        .with_entry(entry)
        .with_file_modes(file_modes)
        .with_flags(flags)
        .with_diagnostics(diagnostics)
        .with_editor(editor)
        .with_shader(family)
        .with_cli(cli),
    )
}

/// Extracts metadata and the four mandatory executable phase entrypoints.
#[must_use]
pub fn extract_language_metadata(
    session: &mut Session,
    file: &File,
) -> Option<LanguageDefinitionMetadata> {
    let descriptor = extract_language_descriptor(session, file)?;
    let construct = language_construct(session, file)?;
    let entrypoints = extract_entrypoints(session, construct, descriptor.shader.is_some())?;
    Some(LanguageDefinitionMetadata {
        descriptor,
        entrypoints,
    })
}

/// Whether `construct` is a `ShaderLanguage`, after checking it takes no
/// parameters and extends `Language` or `ShaderLanguage` without generics.
fn is_shader_construct(session: &mut Session, construct: &ConstructDecl) -> Option<bool> {
    if construct
        .params
        .as_ref()
        .is_some_and(|params| !params.is_empty())
    {
        session.report(
            Diagnostic::error_at(
                "KLF-DEF-0001",
                construct.span,
                "Language constructs cannot take parameters",
            )
            .with_label(construct.span, "remove the construct parameters"),
        );
        return None;
    }
    let shader = match construct.extends.as_slice() {
        [family] if construct.generics.is_empty() => is_shader_family(family),
        _ => None,
    };
    if shader.is_none() {
        session.report(
            Diagnostic::error_at(
                "KLF-DEF-0001",
                construct.span,
                "construct must extend Language or ShaderLanguage without generic parameters",
            )
            .with_label(construct.span, "expected `extends Language<Tree, Program>`"),
        );
    }
    shader
}

/// The metadata fields `construct` initializes, by name.
fn metadata_fields<'a>(
    session: &mut Session,
    construct: &'a ConstructDecl,
) -> Option<MetadataFields<'a>> {
    let mut fields = MetadataFields::new();
    for member in &construct.members {
        let CMember::Member(member) = member else {
            continue;
        };
        let MemberKind::Field { name, init, .. } = &member.kind else {
            continue;
        };
        let Some(FieldInit::Expr(value)) = init else {
            continue;
        };
        if fields.insert(name.name.as_str(), value).is_some() {
            session.report(
                Diagnostic::error_at(
                    "KLF-DEF-0002",
                    name.span,
                    format!("duplicate Language metadata field `{}`", name.name),
                )
                .with_label(name.span, "keep one metadata field"),
            );
        }
    }
    (session.error_count() == 0).then_some(fields)
}

fn language_construct<'a>(session: &mut Session, file: &'a File) -> Option<&'a ConstructDecl> {
    let constructs: Vec<&ConstructDecl> = file
        .decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Construct(construct) => Some(construct),
            _ => None,
        })
        .collect();
    if constructs.len() != 1 {
        session.report(Diagnostic::error(
            "KLF-DEF-0001",
            "definition must contain exactly one Language construct",
        ));
        return None;
    }
    Some(constructs[0])
}

/// Whether `ty` names `Language` (false) or `ShaderLanguage` (true); `None`
/// for anything else.
fn is_shader_family(ty: &klf_kira_syntax::ast::Type) -> Option<bool> {
    let TypeKind::Named { path, .. } = &ty.kind else {
        return None;
    };
    match path.segments.as_slice() {
        [name] if name.name.as_str() == "Language" => Some(false),
        [name] if name.name.as_str() == "ShaderLanguage" => Some(true),
        _ => None,
    }
}

fn required<'a>(
    session: &mut Session,
    fields: &MetadataFields<'a>,
    name: &str,
) -> Option<&'a Expr> {
    let Some(value) = fields.get(name).copied() else {
        session.report(Diagnostic::error(
            "KLF-DEF-0002",
            format!("Language metadata is missing required field `{name}`"),
        ));
        return None;
    };
    Some(value)
}

fn required_string(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<String> {
    let value = required(session, fields, name)?;
    if let ExprKind::Str(value) = &value.kind {
        return Some(value.clone());
    }
    session.report(
        Diagnostic::error_at(
            "KLF-DEF-0002",
            value.span,
            format!("Language metadata field `{name}` must be a string"),
        )
        .with_label(value.span, "expected a string literal"),
    );
    None
}

fn required_strings(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<Vec<String>> {
    let value = required(session, fields, name)?;
    let ExprKind::ArrayLit(values) = &value.kind else {
        session.report(
            Diagnostic::error_at(
                "KLF-DEF-0002",
                value.span,
                format!("Language metadata field `{name}` must be an array of strings"),
            )
            .with_label(value.span, "expected an array literal"),
        );
        return None;
    };
    let mut strings = Vec::with_capacity(values.len());
    for item in values {
        let ExprKind::Str(string) = &item.kind else {
            session.report(
                Diagnostic::error_at(
                    "KLF-DEF-0002",
                    item.span,
                    format!("Language metadata field `{name}` must contain only strings"),
                )
                .with_label(item.span, "expected a string literal"),
            );
            return None;
        };
        strings.push(string.clone());
    }
    Some(strings)
}

fn required_memory(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<MemoryModel> {
    match enum_name(session, fields, name)?.as_str() {
        "Unmanaged" => Some(MemoryModel::Unmanaged),
        "Counted" => Some(MemoryModel::Counted),
        "Affine" => Some(MemoryModel::Affine),
        value => invalid_enum(session, name, value),
    }
}

fn required_exclusivity(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<Exclusivity> {
    match enum_name(session, fields, name)?.as_str() {
        "Static" => Some(Exclusivity::Static),
        "Dynamic" => Some(Exclusivity::Dynamic),
        "None" => Some(Exclusivity::None),
        value => invalid_enum(session, name, value),
    }
}

fn required_convention(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<DefaultConvention> {
    match enum_name(session, fields, name)?.as_str() {
        "Ridl" => Some(DefaultConvention::Ridl),
        "Platform" => Some(DefaultConvention::Platform),
        value => invalid_enum(session, name, value),
    }
}

/// The optional `entry` member; omitted, a program reports its outcome.
fn optional_entry(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    name: &str,
) -> Option<EntryConvention> {
    if !fields.contains_key(name) {
        return Some(EntryConvention::Outcome);
    }
    match enum_name(session, fields, name)?.as_str() {
        "Outcome" => Some(EntryConvention::Outcome),
        "Process" => Some(EntryConvention::Process),
        "ProcessExit" => Some(EntryConvention::ProcessExit),
        value => invalid_enum(session, name, value),
    }
}

/// The optional `entry` member; omitted, a program reports its outcome.
fn enum_name(session: &mut Session, fields: &MetadataFields<'_>, name: &str) -> Option<String> {
    let value = required(session, fields, name)?;
    let ExprKind::EnumLit {
        name: variant,
        args,
    } = &value.kind
    else {
        session.report(
            Diagnostic::error_at(
                "KLF-DEF-0002",
                value.span,
                format!("Language metadata field `{name}` must be an enum case"),
            )
            .with_label(value.span, "expected `.Case`"),
        );
        return None;
    };
    if args.is_some() {
        session.report(
            Diagnostic::error_at(
                "KLF-DEF-0002",
                value.span,
                format!("Language metadata field `{name}` cannot carry a payload"),
            )
            .with_label(value.span, "remove the case payload"),
        );
        return None;
    }
    Some(variant.name.as_str().to_owned())
}

fn invalid_enum<T>(session: &mut Session, field: &str, value: &str) -> Option<T> {
    session.report(Diagnostic::error(
        "KLF-DEF-0002",
        format!("unknown value `.{value}` for Language metadata field `{field}`"),
    ));
    None
}

/// The memory model, exclusivity, convention and std root. A shader
/// language has none of them (`KLF-KSL.md` section 5.1), so its descriptor
/// keeps inert values.
fn language_model(
    session: &mut Session,
    fields: &MetadataFields<'_>,
    shader: bool,
) -> Option<(MemoryModel, Exclusivity, DefaultConvention, String)> {
    if shader {
        return Some((
            MemoryModel::Unmanaged,
            Exclusivity::None,
            DefaultConvention::Platform,
            String::new(),
        ));
    }
    Some((
        required_memory(session, fields, "memory")?,
        required_exclusivity(session, fields, "exclusivity")?,
        required_convention(session, fields, "convention")?,
        required_string(session, fields, "stdRoot")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_core::FileId;
    use klf_kira_syntax::parse;
    use klf_services::{DriverFlag, DriverFlagValue};
    use klf_source::SourceFile;
    use klf_target::Target;

    fn target() -> Target {
        "aarch64-apple-darwin".parse().expect("valid target")
    }

    #[test]
    fn extracts_required_language_metadata() {
        let source = r#"
construct Calc() extends Language<Tree, Program> {
    let name: String = "calc"
    let driver: String = "calc-driver"
    let version: String = "1.0.0"
    let extensions: [String] = ["calc", "clc"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Platform
    let stdRoot: String = "std"
    let flags: [DriverFlag] = [
        DriverFlag { name = "warnings", default_value = .Boolean(false) },
        DriverFlag { name = "jobs", default_value = .Integer(4) },
        DriverFlag { name = "sysroot", default_value = .String("std") }
    ]
    function units(session: Session, inputs: [SourceFile]) -> [Unit] { return [] }
    function parse(session: Session, unit: Unit) -> Tree { return [] }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program { return [] }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule { return [] }
    function compatDriver(arguments: [String]) -> Optional<DriverRequest> { return .None }
}
"#;
        let mut session = Session::new(target());
        let file = SourceFile::new(FileId::from_u32(0), "calc.klfd", source);
        let parsed = parse(&file);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

        let descriptor = extract_language_descriptor(&mut session, &parsed.file)
            .expect("valid Language metadata");
        assert_eq!(descriptor.name, "calc");
        assert_eq!(descriptor.driver, "calc-driver");
        assert_eq!(descriptor.extensions, vec!["calc", "clc"]);
        assert_eq!(descriptor.memory, MemoryModel::Counted);
        assert_eq!(descriptor.convention, DefaultConvention::Platform);
        assert_eq!(
            descriptor.flags,
            vec![
                DriverFlag {
                    name: "warnings".to_owned(),
                    default_value: DriverFlagValue::Boolean(false),
                },
                DriverFlag {
                    name: "jobs".to_owned(),
                    default_value: DriverFlagValue::Integer(4),
                },
                DriverFlag {
                    name: "sysroot".to_owned(),
                    default_value: DriverFlagValue::String("std".to_owned()),
                },
            ]
        );

        let metadata = extract_language_metadata(&mut session, &parsed.file)
            .expect("valid Language metadata and phases");
        assert!(metadata.entrypoints.compat_driver.is_some());
        assert_eq!(metadata.entrypoints.units.file, FileId::from_u32(0));
        assert!(metadata.entrypoints.lower.start > metadata.entrypoints.units.start);

        let invalid_source = source.replace("inputs: [SourceFile]", "inputs: Unit");
        let mut invalid_session = Session::new(target());
        let invalid_file = SourceFile::new(FileId::from_u32(0), "invalid.klfd", invalid_source);
        let invalid_parsed = parse(&invalid_file);
        assert!(extract_language_metadata(&mut invalid_session, &invalid_parsed.file).is_none());
        assert_eq!(invalid_session.diagnostics()[0].code, "KLF-DEF-0004");
    }

    #[test]
    fn rejects_a_definition_without_one_language_construct() {
        let mut session = Session::new(target());
        let file = SourceFile::new(FileId::from_u32(0), "calc.klfd", "function main() {}");
        let parsed = parse(&file);
        assert!(extract_language_descriptor(&mut session, &parsed.file).is_none());
        assert_eq!(session.diagnostics()[0].code, "KLF-DEF-0001");
    }
}
