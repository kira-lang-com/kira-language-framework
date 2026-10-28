//! Reading the command line a definition declares (`KLF-CLI.md` section 3)
//! out of its syntax: `ParsableCommand` structs, their `@Option`, `@Flag` and
//! `@Argument` properties, and `CommandConfiguration` literals. Nothing here
//! runs; every value the parser shows must be a literal.

use super::{ARGUMENT, FLAG, OPTION, PARSABLE_COMMAND};
use crate::wrappers::annotation_name;
use klf_args::{Action, Arg, ArgKind, Arity, Command, ValueType, kebab_case};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{
    Annotation, AnnotationArgs, CasePayload, DeclKind, EnumItem, Expr, ExprKind, FieldInit, File,
    FunctionDecl, Ident, Member, MemberKind, Param, StructDecl, Type, TypeKind, UnaryOp,
};
use klf_services::Session;
use klf_source::Span;
use std::collections::HashMap;

/// The type one command-line value converts to.
#[derive(Clone, Debug)]
pub(crate) enum Scalar {
    Bool,
    Int,
    String,
    /// A payload-free enum; the value is the case's index.
    Choice(Name, Vec<Name>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shape {
    Single,
    Optional,
    List,
}

/// One stored property of a command.
#[derive(Clone, Debug)]
pub(crate) struct CliField {
    pub name: Name,
    pub ty: Type,
    pub init: Option<Expr>,
    /// The argument slot, scalar and shape; `None` for a plain property.
    pub value: Option<(u32, Scalar, Shape)>,
}

/// One `ParsableCommand` struct.
#[derive(Clone, Debug)]
pub(crate) struct CommandDecl {
    pub name: Name,
    pub index: u32,
    pub fields: Vec<CliField>,
    pub runs: bool,
    /// The command without its subcommands, which [`tree`] resolves.
    pub command: Command,
    pub subcommands: Vec<Ident>,
    pub default_subcommand: Option<Ident>,
}

#[derive(Default)]
struct Config {
    name: Option<String>,
    abstract_text: String,
    discussion: String,
    aliases: Vec<String>,
    subcommands: Vec<Ident>,
    default_subcommand: Option<Ident>,
    hidden: bool,
}

fn report(session: &mut Session, span: Span, message: impl Into<String>) {
    let message = message.into();
    session.report(
        Diagnostic::error_at("KLF-DEF-0050", span, message.clone()).with_label(span, message),
    );
}

pub(crate) fn conforms(structure: &StructDecl) -> bool {
    structure
        .conforms
        .iter()
        .any(|ty| is_named(ty, PARSABLE_COMMAND))
}

fn is_named(ty: &Type, expected: &str) -> bool {
    matches!(&ty.kind, TypeKind::Named { path, args }
        if args.is_empty() && path.segments.len() == 1 && path.segments[0].name.as_str() == expected)
}

/// The payload-free, non-generic enums of `file`, with their case names.
fn choice_enums(file: &File) -> HashMap<Name, Vec<Name>> {
    file.decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Enum(declared) if declared.generics.is_empty() => {
                let mut cases = Vec::new();
                for item in &declared.items {
                    match item {
                        EnumItem::Case(case) if matches!(case.payload, CasePayload::None) => {
                            cases.push(case.name.name.clone());
                        }
                        EnumItem::Case(_) => return None,
                        EnumItem::Method(_) => {}
                    }
                }
                Some((declared.name.name.clone(), cases))
            }
            _ => None,
        })
        .collect()
}

/// Every `ParsableCommand` struct of `file`, numbered in declaration order.
pub(crate) fn commands(session: &mut Session, file: &File) -> Option<Vec<CommandDecl>> {
    let enums = choice_enums(file);
    let mut out = Vec::new();
    let errors = session.error_count();
    for decl in &file.decls {
        let DeclKind::Struct(structure) = &decl.kind else {
            continue;
        };
        if !conforms(structure) {
            continue;
        }
        let index = u32::try_from(out.len()).unwrap_or(u32::MAX);
        if let Some(command) = command(session, structure, index, &enums) {
            out.push(command);
        }
    }
    (session.error_count() == errors).then_some(out)
}

fn command(
    session: &mut Session,
    structure: &StructDecl,
    index: u32,
    enums: &HashMap<Name, Vec<Name>>,
) -> Option<CommandDecl> {
    let mut config = Config::default();
    let mut fields = Vec::new();
    let mut args = Vec::new();
    let mut runs = false;
    for member in &structure.members {
        match &member.kind {
            MemberKind::Field { name, init, .. } if name.name.as_str() == "configuration" => {
                let Some(FieldInit::Expr(value)) = init else {
                    report(
                        session,
                        member.span,
                        "`configuration` needs a CommandConfiguration literal",
                    );
                    return None;
                };
                config = configuration(session, value)?;
            }
            MemberKind::Field { name, ty, init, .. } => {
                fields.push(field(
                    session,
                    member,
                    name,
                    ty,
                    init.as_ref(),
                    &mut args,
                    enums,
                )?);
            }
            MemberKind::Method(function) if function.name.name.as_str() == "run" => {
                check_run(session, function)?;
                runs = true;
            }
            _ => {}
        }
    }
    let name = config
        .name
        .unwrap_or_else(|| kebab_case(structure.name.name.as_str()));
    let command = Command {
        name,
        abstract_text: config.abstract_text,
        discussion: config.discussion,
        aliases: config.aliases,
        args,
        subcommands: Vec::new(),
        default_subcommand: None,
        action: if runs {
            Action::Definition(index)
        } else {
            Action::None
        },
        hidden: config.hidden,
        version: None,
    };
    Some(CommandDecl {
        name: structure.name.name.clone(),
        index,
        fields,
        runs,
        command,
        subcommands: config.subcommands,
        default_subcommand: config.default_subcommand,
    })
}

/// The command-line argument a wrapped property declares.
/// The command property `member`, named `name` of type `ty`, adding the
/// argument it declares to `args`.
fn field(
    session: &mut Session,
    member: &Member,
    name: &Ident,
    ty: &Type,
    init: Option<&FieldInit>,
    args: &mut Vec<Arg>,
    enums: &HashMap<Name, Vec<Name>>,
) -> Option<CliField> {
    let init = match init {
        Some(FieldInit::Expr(value)) => Some(value.clone()),
        Some(FieldInit::Block(_)) => {
            report(
                session,
                member.span,
                "a command's properties are stored, not computed",
            );
            return None;
        }
        None => None,
    };
    let wrapper = member.annotations.iter().find(|annotation| {
        matches!(
            annotation_name(&annotation.path),
            Some(OPTION | FLAG | ARGUMENT)
        )
    });
    let value = match wrapper {
        Some(annotation) => {
            let slot = u32::try_from(args.len()).unwrap_or(u32::MAX);
            let (arg, scalar, shape) =
                argument(session, annotation, name, ty, init.as_ref(), enums)?;
            args.push(arg);
            Some((slot, scalar, shape))
        }
        None if init.is_none() => {
            report(
                session,
                member.span,
                format!(
                    "`{}` is neither an @Option, @Flag or @Argument nor initialized",
                    name.name
                ),
            );
            return None;
        }
        None => None,
    };
    Some(CliField {
        name: name.name.clone(),
        ty: ty.clone(),
        init,
        value,
    })
}

/// Checks that `run` is `function run(self) -> Int` with a body.
fn check_run(session: &mut Session, function: &FunctionDecl) -> Option<()> {
    let receiver = matches!(function.params.as_slice(), [Param::SelfParam { .. }]);
    let returns_int = function.ret.as_ref().is_some_and(|ty| is_named(ty, "Int"));
    if !receiver || !returns_int || function.body.is_none() {
        report(
            session,
            function.span,
            "`run` must be `function run(self) -> Int` with a body",
        );
        return None;
    }
    Some(())
}

fn argument(
    session: &mut Session,
    annotation: &Annotation,
    name: &Ident,
    ty: &Type,
    init: Option<&Expr>,
    enums: &HashMap<Name, Vec<Name>>,
) -> Option<(Arg, Scalar, Shape)> {
    let wrapper = annotation_name(&annotation.path).unwrap_or_default();
    let kind = match wrapper {
        OPTION => ArgKind::Option,
        FLAG => ArgKind::Flag,
        _ => ArgKind::Positional,
    };
    let (shape, element) = shape_of(ty);
    let Some(scalar) = scalar_of(element, enums) else {
        report(
            session,
            ty.span,
            "a command-line value is Bool, Int, String or a payload-free enum, optionally in [ ] or Optional< >",
        );
        return None;
    };
    if kind == ArgKind::Flag
        && (shape != Shape::Single || !matches!(scalar, Scalar::Bool | Scalar::Int))
    {
        report(
            session,
            ty.span,
            "an @Flag is a Bool, or an Int that counts occurrences",
        );
        return None;
    }
    let value = match &scalar {
        Scalar::Bool => ValueType::Bool,
        Scalar::Int => ValueType::Int,
        Scalar::String => ValueType::String,
        Scalar::Choice(_, cases) => {
            ValueType::Choice(cases.iter().map(|case| kebab_case(case.as_str())).collect())
        }
    };
    let mut arg = Arg::new(name.name.as_str(), kind, value);
    if shape == Shape::List {
        arg.arity = Arity::Many;
    }
    arg.required = kind != ArgKind::Flag && shape == Shape::Single && init.is_none();
    if shape == Shape::Single && kind != ArgKind::Flag {
        arg.default = init.and_then(|init| default_text(init, &scalar));
    }
    for (label, value) in annotation_pairs(annotation) {
        match label.name.as_str() {
            "help" => arg.help = string(session, value)?,
            "valueName" => arg.value_name = string(session, value)?,
            "hidden" => arg.hidden = boolean(session, value)?,
            "name" if kind != ArgKind::Positional => {
                let (long, short) = names(session, value, name)?;
                arg.long = long;
                arg.short = short;
            }
            other => {
                report(
                    session,
                    label.span,
                    format!("`@{wrapper}` has no static field `{other}`"),
                );
                return None;
            }
        }
    }
    Some((arg, scalar, shape))
}

fn annotation_pairs(annotation: &Annotation) -> Vec<(Ident, &Expr)> {
    match &annotation.args {
        AnnotationArgs::None => Vec::new(),
        AnnotationArgs::Fields(pairs) => pairs.iter().map(|(k, v)| (k.clone(), v)).collect(),
        AnnotationArgs::Call(args) => args
            .iter()
            .filter_map(|arg| arg.label.clone().map(|label| (label, &arg.value)))
            .collect(),
    }
}

/// The long and short names a `NameSpecification` case gives.
fn names(
    session: &mut Session,
    value: &Expr,
    property: &Ident,
) -> Option<(Option<String>, Option<char>)> {
    let long = || Some(kebab_case(property.name.as_str()));
    let short = || property.name.as_str().chars().next();
    let ExprKind::EnumLit { name, args } = &value.kind else {
        report(session, value.span, "`name` is a NameSpecification case");
        return None;
    };
    let texts: Vec<&Expr> = args.iter().flatten().map(|arg| &arg.value).collect();
    let letter = |session: &mut Session, value: &Expr| {
        let text = string(session, value)?;
        let mut letters = text.chars();
        if let (Some(letter), None) = (letters.next(), letters.next()) {
            Some(letter)
        } else {
            report(session, value.span, "a short name is one character");
            None
        }
    };
    match (name.name.as_str(), texts.as_slice()) {
        ("Long", []) => Some((long(), None)),
        ("Short", []) => Some((None, short())),
        ("ShortAndLong", []) => Some((long(), short())),
        ("CustomLong", [text]) => Some((Some(string(session, text)?), None)),
        ("CustomShort", [text]) => Some((None, Some(letter(session, text)?))),
        ("Custom", [long_name, short_name]) => Some((
            Some(string(session, long_name)?),
            Some(letter(session, short_name)?),
        )),
        _ => {
            report(
                session,
                value.span,
                "`name` is .Long, .Short, .ShortAndLong, .CustomLong(\"x\"), .CustomShort(\"x\") or .Custom(\"long\", \"x\")",
            );
            None
        }
    }
}

fn shape_of(ty: &Type) -> (Shape, &Type) {
    match &ty.kind {
        TypeKind::Array(element) => (Shape::List, element),
        TypeKind::Named { path, args }
            if path.segments.len() == 1
                && path.segments[0].name.as_str() == "Optional"
                && args.len() == 1 =>
        {
            (Shape::Optional, &args[0])
        }
        _ => (Shape::Single, ty),
    }
}

fn scalar_of(ty: &Type, enums: &HashMap<Name, Vec<Name>>) -> Option<Scalar> {
    let TypeKind::Named { path, args } = &ty.kind else {
        return None;
    };
    let [segment] = path.segments.as_slice() else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    match segment.name.as_str() {
        "Bool" => Some(Scalar::Bool),
        "Int" => Some(Scalar::Int),
        "String" => Some(Scalar::String),
        _ => enums
            .get(&segment.name)
            .filter(|cases| !cases.is_empty())
            .map(|cases| Scalar::Choice(segment.name.clone(), cases.clone())),
    }
}

/// The default as help shows it, when the initial value is a literal.
fn default_text(init: &Expr, scalar: &Scalar) -> Option<String> {
    match (&init.kind, scalar) {
        (ExprKind::Int(text), Scalar::Int) => Some(text.to_string()),
        (
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            },
            Scalar::Int,
        ) => match &operand.kind {
            ExprKind::Int(text) => Some(format!("-{text}")),
            _ => None,
        },
        (ExprKind::Str(text), Scalar::String) => Some(text.clone()),
        (ExprKind::Bool(value), Scalar::Bool) => Some(value.to_string()),
        (ExprKind::EnumLit { name, args: None }, Scalar::Choice(..)) => {
            Some(kebab_case(name.name.as_str()))
        }
        _ => None,
    }
}

fn string(session: &mut Session, value: &Expr) -> Option<String> {
    if let ExprKind::Str(text) = &value.kind {
        return Some(text.clone());
    }
    report(session, value.span, "expected a string literal");
    None
}

fn boolean(session: &mut Session, value: &Expr) -> Option<bool> {
    if let ExprKind::Bool(value) = &value.kind {
        return Some(*value);
    }
    report(session, value.span, "expected `true` or `false`");
    None
}

fn configuration(session: &mut Session, value: &Expr) -> Option<Config> {
    let ExprKind::StructLit { ty, fields } = &value.kind else {
        report(
            session,
            value.span,
            "`configuration` needs a CommandConfiguration literal",
        );
        return None;
    };
    if !is_named(ty, "CommandConfiguration") {
        report(
            session,
            value.span,
            "`configuration` needs a CommandConfiguration literal",
        );
        return None;
    }
    let mut config = Config::default();
    for (field, value) in fields {
        match field.name.as_str() {
            "commandName" => config.name = Some(string(session, value)?),
            "abstract" => config.abstract_text = string(session, value)?,
            "discussion" => config.discussion = string(session, value)?,
            "hidden" => config.hidden = boolean(session, value)?,
            "aliases" => {
                let ExprKind::ArrayLit(items) = &value.kind else {
                    report(
                        session,
                        value.span,
                        "`aliases` is an array of string literals",
                    );
                    return None;
                };
                for item in items {
                    config.aliases.push(string(session, item)?);
                }
            }
            "subcommands" => {
                let ExprKind::ArrayLit(items) = &value.kind else {
                    report(
                        session,
                        value.span,
                        "`subcommands` is an array of command type names",
                    );
                    return None;
                };
                for item in items {
                    config.subcommands.push(type_name(session, item)?);
                }
            }
            "defaultSubcommand" => config.default_subcommand = Some(type_name(session, value)?),
            other => {
                report(
                    session,
                    field.span,
                    format!("CommandConfiguration has no field `{other}`"),
                );
                return None;
            }
        }
    }
    Some(config)
}

fn type_name(session: &mut Session, value: &Expr) -> Option<Ident> {
    if let ExprKind::Path(path) = &value.kind
        && let [segment] = path.segments.as_slice()
    {
        return Some(segment.clone());
    }
    report(
        session,
        value.span,
        "expected the name of a ParsableCommand struct",
    );
    None
}

/// The commands `names` refer to, with their subcommands resolved.
fn resolve(
    session: &mut Session,
    names: &[Ident],
    default: Option<&Ident>,
    commands: &HashMap<Name, &CommandDecl>,
    stack: &mut Vec<Name>,
) -> Option<(Vec<Command>, Option<String>)> {
    let mut resolved = Vec::new();
    let mut default_name = None;
    for name in names {
        let Some(decl) = commands.get(&name.name) else {
            report(
                session,
                name.span,
                format!("`{}` is not a ParsableCommand struct", name.name),
            );
            return None;
        };
        if stack.contains(&name.name) {
            report(
                session,
                name.span,
                format!("`{}` contains itself", name.name),
            );
            return None;
        }
        stack.push(name.name.clone());
        let (subcommands, inner_default) = resolve(
            session,
            &decl.subcommands,
            decl.default_subcommand.as_ref(),
            commands,
            stack,
        )?;
        stack.pop();
        let mut command = decl.command.clone();
        command.subcommands = subcommands;
        command.default_subcommand = inner_default;
        if default.is_some_and(|default| default.name == name.name) {
            default_name = Some(command.name.clone());
        }
        resolved.push(command);
    }
    if let Some(default) = default
        && default_name.is_none()
    {
        report(
            session,
            default.span,
            "the default subcommand must be one of `subcommands`",
        );
        return None;
    }
    Some((resolved, default_name))
}

/// The Language's own command line: its `configuration` and the options its
/// `@Flag` and `@Option` members declare. Built-in commands are the kernel's
/// and are added by the driver.
pub(crate) fn language_command(
    session: &mut Session,
    file: &File,
    members: &[&Member],
    driver: &str,
) -> Option<(Command, Vec<klf_services::DriverFlag>)> {
    let decls = commands(session, file)?;
    let by_name: HashMap<Name, &CommandDecl> =
        decls.iter().map(|decl| (decl.name.clone(), decl)).collect();
    let mut root = Command::new(driver);
    let mut config = Config::default();
    let mut flags = Vec::new();
    for member in members {
        let MemberKind::Field { name, ty, init, .. } = &member.kind else {
            continue;
        };
        if name.name.as_str() == "configuration" {
            let Some(FieldInit::Expr(value)) = init else {
                report(
                    session,
                    member.span,
                    "`configuration` needs a CommandConfiguration literal",
                );
                return None;
            };
            config = configuration(session, value)?;
            continue;
        }
        let Some(annotation) = member
            .annotations
            .iter()
            .find(|annotation| matches!(annotation_name(&annotation.path), Some(OPTION | FLAG)))
        else {
            continue;
        };
        let init = match init {
            Some(FieldInit::Expr(value)) => Some(value),
            _ => None,
        };
        let (arg, scalar, shape) = argument(session, annotation, name, ty, init, &HashMap::new())?;
        let default = init.map(|init| (&init.kind, &scalar));
        let value = match default {
            Some((ExprKind::Bool(value), Scalar::Bool)) if shape == Shape::Single => {
                klf_services::DriverFlagValue::Boolean(*value)
            }
            Some((ExprKind::Str(text), Scalar::String)) if shape == Shape::Single => {
                klf_services::DriverFlagValue::String(text.clone())
            }
            // A repeatable option, `-DNAME -DOTHER`: the definition reads its
            // values as one String, one value per line.
            Some((ExprKind::ArrayLit(items), Scalar::String))
                if shape == Shape::List && items.is_empty() =>
            {
                klf_services::DriverFlagValue::String(String::new())
            }
            Some((_, Scalar::Int)) if shape == Shape::Single => {
                let Some(text) = default_text(init?, &scalar) else {
                    report(
                        session,
                        member.span,
                        "a Language option's initial value is a literal",
                    );
                    return None;
                };
                klf_services::DriverFlagValue::Integer(text.parse().ok()?)
            }
            _ => {
                report(
                    session,
                    member.span,
                    "a Language option is a Bool, Int or String with a literal initial value, or a [String] starting empty",
                );
                return None;
            }
        };
        let Some(long) = arg.long.clone() else {
            report(session, member.span, "a Language option needs a long name");
            return None;
        };
        let mut arg = arg;
        arg.id.clone_from(&long);
        arg.required = false;
        root.args.push(arg);
        flags.push(klf_services::DriverFlag {
            name: long,
            default_value: value,
        });
    }
    root.abstract_text = config.abstract_text;
    root.discussion = config.discussion;
    let (subcommands, _) = resolve(
        session,
        &config.subcommands,
        None,
        &by_name,
        &mut Vec::new(),
    )?;
    root.subcommands = subcommands;
    Some((root, flags))
}
