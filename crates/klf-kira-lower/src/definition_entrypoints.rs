//! The phases and hooks of a `Language` construct, each checked against
//! the signature the kernel calls it with.

use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{
    CMember, ConstructDecl, DeclKind, File, FunctionDecl, MemberKind, Param, Type, TypeKind,
};
use klf_services::Session;

use crate::definition_metadata::DefinitionEntrypoints;

pub(crate) fn extract_entrypoints(
    session: &mut Session,
    construct: &ConstructDecl,
    shader: bool,
) -> Option<DefinitionEntrypoints> {
    let mut spans = [None; 4];
    let mut compat_driver = None;
    let mut run_program = None;
    let mut emit = None;
    let mut format = None;
    let mut project = None;
    for member in &construct.members {
        let CMember::Member(member) = member else {
            continue;
        };
        let MemberKind::Method(function) = &member.kind else {
            continue;
        };
        let (slot, kind) = match function.name.name.as_str() {
            "compatDriver" => (&mut compat_driver, "compatDriver override"),
            "runProgram" => (&mut run_program, "runProgram hook"),
            "emit" => (&mut emit, "emit hook"),
            "format" => (&mut format, "format hook"),
            "project" => (&mut project, "project hook"),
            _ => (&mut None, ""),
        };
        if !kind.is_empty() {
            if slot.is_some() {
                report_signature_error(session, function, &format!("duplicate {kind}"));
            } else if function.body.is_none() {
                report_signature_error(session, function, &format!("{kind} needs a body"));
            } else if match function.name.name.as_str() {
                "compatDriver" => validate_compat_driver_signature(session, function),
                "runProgram" => validate_run_program_signature(session, function),
                "emit" => validate_emit_signature(session, function, shader),
                "project" => validate_project_signature(session, function),
                _ => validate_format_signature(session, function),
            } {
                *slot = Some(function.span);
            }
            continue;
        }
        let Some(index) = ["units", "parse", "elaborate", "lower"]
            .iter()
            .position(|name| *name == function.name.name.as_str())
        else {
            continue;
        };
        if spans[index].is_some() {
            session.report(
                Diagnostic::error_at(
                    "KLF-DEF-0003",
                    function.name.span,
                    format!(
                        "duplicate Language phase entrypoint `{}`",
                        function.name.name
                    ),
                )
                .with_label(function.name.span, "keep one phase entrypoint"),
            );
        } else if function.body.is_none() {
            session.report(
                Diagnostic::error_at(
                    "KLF-DEF-0003",
                    function.name.span,
                    format!(
                        "Language phase entrypoint `{}` needs a body",
                        function.name.name
                    ),
                )
                .with_label(function.name.span, "define the phase body"),
            );
        } else if validate_phase_signature(session, function, index, shader) {
            spans[index] = Some(function.span);
        }
    }
    if session.error_count() != 0 {
        return None;
    }
    let [Some(units), Some(parse), Some(elaborate), Some(lower)] = spans else {
        session.report(Diagnostic::error(
            "KLF-DEF-0003",
            "Language construct must define units, parse, elaborate and lower",
        ));
        return None;
    };
    Some(DefinitionEntrypoints {
        units,
        parse,
        elaborate,
        lower,
        compat_driver,
        run_program,
        emit,
        format,
        project,
    })
}

/// Whether a `Language` construct of `file` writes the member `name`.
pub(crate) fn has_hook(file: &File, name: &str) -> bool {
    file.decls.iter().any(|decl| {
        let DeclKind::Construct(construct) = &decl.kind else {
            return false;
        };
        construct.members.iter().any(|member| {
            matches!(member, CMember::Member(member)
                if matches!(&member.kind, MemberKind::Method(function) if function.name.name.as_str() == name))
        })
    })
}

fn validate_phase_signature(
    session: &mut Session,
    function: &FunctionDecl,
    phase: usize,
    shader: bool,
) -> bool {
    if !function.generics.is_empty() {
        report_signature_error(session, function, "phase entrypoints cannot be generic");
        return false;
    }
    let expected = match phase {
        0 => (&["Session", "[SourceFile]"][..], "[Unit]"),
        // A parse takes the program's files too (`docs/conformers.md`).
        1 => (&["Session", "Unit", "[SourceFile]"][..], "Tree"),
        2 => (&["Session", "Unit", "Tree"][..], "Program"),
        3 if shader => (&["Session", "Unit", "Program"][..], "ShaderModule"),
        3 => (&["Session", "Unit", "Program"][..], "KirModule"),
        _ => unreachable!("Language phase index is bounded"),
    };
    if function.params.len() != expected.0.len()
        || function
            .params
            .iter()
            .zip(expected.0)
            .any(|(param, expected)| !param_type_matches(param, expected))
        || function
            .ret
            .as_ref()
            .is_none_or(|ret| !type_matches(ret, expected.1))
    {
        report_signature_error(
            session,
            function,
            "phase entrypoint has the wrong signature",
        );
        return false;
    }
    true
}

fn validate_run_program_signature(session: &mut Session, function: &FunctionDecl) -> bool {
    let valid_parameters = matches!(
        function.params.as_slice(),
        [command, arguments]
            if param_type_matches(command, "[String]") && param_type_matches(arguments, "[String]")
    );
    let valid_result = function
        .ret
        .as_ref()
        .is_some_and(|result| type_matches(result, "Int"));
    if function.generics.is_empty() && valid_parameters && valid_result {
        return true;
    }
    report_signature_error(
        session,
        function,
        "runProgram must have signature `runProgram(command: [String], driverArguments: [String]) -> Int`",
    );
    false
}

fn validate_emit_signature(session: &mut Session, function: &FunctionDecl, shader: bool) -> bool {
    let valid = matches!(function.params.as_slice(), [unit] if param_type_matches(unit, "ShaderUnit"))
        && function
            .ret
            .as_ref()
            .is_some_and(|result| type_matches(result, "Bool"));
    if shader && function.generics.is_empty() && valid {
        return true;
    }
    report_signature_error(
        session,
        function,
        "emit is a ShaderLanguage hook with signature `emit(unit: ShaderUnit) -> Bool`",
    );
    false
}

fn validate_format_signature(session: &mut Session, function: &FunctionDecl) -> bool {
    let valid = function.generics.is_empty()
        && matches!(
            function.params.as_slice(),
            [session, unit, tree]
                if param_type_matches(session, "Session")
                    && param_type_matches(unit, "Unit")
                    && param_type_matches(tree, "Tree")
        )
        && function
            .ret
            .as_ref()
            .is_some_and(|result| type_matches(result, "Bool"));
    if !valid {
        report_signature_error(
            session,
            function,
            "format must have signature `format(session: Session, unit: Unit, tree: Tree) -> Bool`",
        );
    }
    valid
}

fn validate_project_signature(session: &mut Session, function: &FunctionDecl) -> bool {
    let valid = function.generics.is_empty()
        && matches!(
            function.params.as_slice(),
            [session, path]
                if param_type_matches(session, "Session") && param_type_matches(path, "String")
        )
        && function.ret.as_ref().is_some_and(|result| {
            matches!(
                &result.kind,
                TypeKind::Named { path, args }
                    if path.segments.len() == 1
                        && path.segments[0].name.as_str() == "Optional"
                        && args.len() == 1
                        && type_matches(&args[0], "Project")
            )
        });
    if !valid {
        report_signature_error(
            session,
            function,
            "project must have signature `project(session: Session, path: String) -> Optional<Project>`",
        );
    }
    valid
}

fn validate_compat_driver_signature(session: &mut Session, function: &FunctionDecl) -> bool {
    let arguments = function.params.as_slice();
    let valid_arguments = matches!(
        arguments,
        [Param::Named(param)]
            if matches!(&param.ty.kind, TypeKind::Array(element) if type_matches(element, "String"))
    );
    let valid_result = function.ret.as_ref().is_some_and(|result| {
        matches!(
            &result.kind,
            TypeKind::Named { path, args }
                if path.segments.len() == 1
                    && path.segments[0].name.as_str() == "Optional"
                    && args.len() == 1
                    && type_matches(&args[0], "DriverRequest")
        )
    });
    if function.generics.is_empty() && valid_arguments && valid_result {
        return true;
    }
    report_signature_error(
        session,
        function,
        "compatDriver must have signature `compatDriver(arguments: [String]) -> Optional<DriverRequest>`",
    );
    false
}

fn param_type_matches(param: &Param, expected: &str) -> bool {
    let Param::Named(param) = param else {
        return false;
    };
    type_matches(&param.ty, expected)
}

fn type_matches(ty: &Type, expected: &str) -> bool {
    if let Some(element) = expected
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return matches!(&ty.kind, TypeKind::Array(inner) if type_matches(inner, element));
    }
    matches!(
        &ty.kind,
        TypeKind::Named { path, args }
            if args.is_empty()
                && path.segments.len() == 1
                && path.segments[0].name.as_str() == expected
    )
}

fn report_signature_error(session: &mut Session, function: &FunctionDecl, message: &str) {
    session.report(
        Diagnostic::error_at("KLF-DEF-0004", function.span, message)
            .with_label(function.span, "match the Language hook ABI"),
    );
}
