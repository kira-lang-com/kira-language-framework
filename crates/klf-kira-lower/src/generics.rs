//! Generic enums, monomorphized.
//!
//! A generic enum declares no type on its own. Every instance written anywhere
//! in the package, such as `Optional<KirValue>`, becomes an ordinary enum
//! before signatures are resolved, so resolution stays a name lookup and
//! nothing below elaboration knows generics exist.
//!
//! An instance is named `Base(Arg,...)`: KIR's text form reads `<` in a
//! nominal name as type arguments, and `FeType::display` spells the name back
//! as `Base<Arg, ...>` for diagnostics.

use crate::enums::{EnumCase, EnumInfo, EnumTable};
use crate::methods::{self, Method};
use crate::structs::resolve_type_with_context;
use crate::ty::FeType;
use klf_core::{FileId, Name};
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{
    Block, CMember, CasePayload, DeclKind, EnumDecl, EnumItem, File, FunctionDecl, MemberKind,
    Param, StmtKind, Type, TypeKind,
};
use klf_services::Session;
use klf_source::SourceFile;
use std::collections::{HashMap, HashSet};

/// The generic enums every package sees (section 6.4). `Optional` lists
/// `None` first; the `compatDriver` entry relies on that tag order.
const PRELUDE: &str = "\
enum Optional<Wrapped> {
    None
    Some(Wrapped)

    function isSome(borrow self) -> Bool {
        match self {
            Some -> return true
            None -> return false
        }
    }

    function isNone(borrow self) -> Bool {
        return !isSome()
    }
}
enum Result<Value, Failure> {
    Ok(Value)
    Error(Failure)
}
";

pub(crate) type Templates = HashMap<Name, EnumDecl>;

pub(crate) fn instance_name(base: &str, arguments: &[FeType]) -> Name {
    let arguments: Vec<String> = arguments.iter().map(FeType::spelling).collect();
    Name::new(format!("{base}({})", arguments.join(",")))
}

pub(crate) fn is_template(enum_decl: &EnumDecl) -> bool {
    !enum_decl.generics.is_empty()
}

/// The prelude's generic enums and the package's own.
pub(crate) fn templates(file: &File) -> Templates {
    let prelude = SourceFile::new(FileId::from_u32(0), "<prelude>", PRELUDE);
    let prelude = klf_kira_syntax::parse(&prelude).file;
    prelude
        .decls
        .iter()
        .chain(&file.decls)
        .filter_map(|decl| match &decl.kind {
            DeclKind::Enum(enum_decl) if is_template(enum_decl) => {
                Some((enum_decl.name.name.clone(), enum_decl.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The enums and methods the package's generic instances produce.
pub(crate) struct Instances {
    pub enums: EnumTable,
    pub methods: Vec<Method>,
}

/// One enum per generic instance the package writes, with the template's
/// methods for it. `enum_names` are the package's own enums, which an
/// instance's arguments may name.
pub(crate) fn instantiate(
    session: &mut Session,
    file: &File,
    templates: &Templates,
    struct_names: &HashSet<Name>,
    enum_names: &HashSet<Name>,
) -> Instances {
    let mut instantiator = Instantiator {
        session,
        templates,
        struct_names,
        enum_names: enum_names.clone(),
        instances: Instances {
            enums: EnumTable::new(),
            methods: Vec::new(),
        },
    };
    for ty in written_types(file) {
        instantiator.visit(ty);
    }
    instantiator.instances
}

struct Instantiator<'a> {
    session: &'a mut Session,
    templates: &'a Templates,
    struct_names: &'a HashSet<Name>,
    /// The package's enums and every instance made so far.
    enum_names: HashSet<Name>,
    instances: Instances,
}

impl Instantiator<'_> {
    fn visit(&mut self, ty: &Type) {
        match &ty.kind {
            TypeKind::Named { path, args } if !args.is_empty() => {
                for argument in args {
                    self.visit(argument);
                }
                let [base] = path.segments.as_slice() else {
                    return;
                };
                if let Some(template) = self.templates.get(&base.name) {
                    self.instantiate(template, args, ty);
                }
            }
            TypeKind::Array(element) => self.visit(element),
            _ => {}
        }
    }

    fn instantiate(&mut self, template: &EnumDecl, args: &[Type], written: &Type) {
        if args.len() != template.generics.len() {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0202",
                    format!(
                        "`{}` takes {} type arguments, got {}",
                        template.name.name,
                        template.generics.len(),
                        args.len()
                    ),
                )
                .with_label(written.span, "wrong type argument count"),
            );
            return;
        }
        let Some(arguments) = args
            .iter()
            .map(|argument| self.resolve(argument, &HashSet::new()))
            .collect::<Option<Vec<FeType>>>()
        else {
            return;
        };
        let name = instance_name(template.name.name.as_str(), &arguments);
        if !self.enum_names.insert(name.clone()) {
            return;
        }
        let parameters: Vec<Name> = template
            .generics
            .iter()
            .map(|generic| generic.name.name.clone())
            .collect();
        let substitutions: HashMap<Name, FeType> =
            parameters.iter().cloned().zip(arguments).collect();
        let parameters: HashSet<Name> = parameters.into_iter().collect();
        let variants = template
            .items
            .iter()
            .filter_map(|item| match item {
                EnumItem::Case(case) => Some(case),
                EnumItem::Method(_) => None,
            })
            .map(|case| EnumCase {
                name: case.name.name.clone(),
                payload: payload_types(&case.payload)
                    .into_iter()
                    .map(|ty| {
                        self.resolve(ty, &parameters)
                            .unwrap_or(FeType::Void)
                            .substitute(&substitutions)
                    })
                    .collect(),
            })
            .collect();
        self.instances.enums.insert(
            name.clone(),
            EnumInfo {
                span: template.span,
                variants,
            },
        );
        self.instantiate_methods(template, args, &FeType::Enum(name));
    }

    /// The template's methods for one instance: each type parameter written in
    /// a signature or a binding annotation becomes the instance's argument,
    /// and any instance that produces is made in turn.
    fn instantiate_methods(&mut self, template: &EnumDecl, args: &[Type], owner: &FeType) {
        let arguments: HashMap<Name, Type> = template
            .generics
            .iter()
            .map(|generic| generic.name.name.clone())
            .zip(args.iter().cloned())
            .collect();
        for item in &template.items {
            let EnumItem::Method(function) = item else {
                continue;
            };
            let mut function = function.clone();
            substitute_function(&mut function, &arguments);
            let mut types = Vec::new();
            function_types(&function, &mut types);
            for ty in types {
                self.visit(ty);
            }
            self.instances
                .methods
                .push(methods::method(owner, &function));
        }
    }

    fn resolve(&mut self, ty: &Type, generics: &HashSet<Name>) -> Option<FeType> {
        resolve_type_with_context(
            self.session,
            ty,
            self.struct_names,
            &self.enum_names,
            generics,
        )
    }
}

pub(crate) fn payload_types(payload: &CasePayload) -> Vec<&Type> {
    match payload {
        CasePayload::None => Vec::new(),
        CasePayload::Tuple(types) => types.iter().collect(),
        CasePayload::Typed { ty, .. } => vec![ty],
    }
}

/// Every type the package spells out: in declarations, and in the
/// annotations of `let` and `var` bindings.
fn written_types(file: &File) -> Vec<&Type> {
    let mut types = Vec::new();
    for decl in &file.decls {
        match &decl.kind {
            DeclKind::Function(function) => function_types(function, &mut types),
            DeclKind::Struct(structure) => {
                for member in &structure.members {
                    member_types(&member.kind, &mut types);
                }
            }
            DeclKind::Enum(enum_decl) if !is_template(enum_decl) => {
                for item in &enum_decl.items {
                    match item {
                        EnumItem::Case(case) => types.extend(payload_types(&case.payload)),
                        EnumItem::Method(function) => function_types(function, &mut types),
                    }
                }
            }
            DeclKind::Extend(extend) => {
                types.push(&extend.ty);
                for member in &extend.items {
                    member_types(&member.kind, &mut types);
                }
            }
            DeclKind::Construct(construct) => {
                for member in &construct.members {
                    if let CMember::Member(member) = member {
                        member_types(&member.kind, &mut types);
                    }
                }
            }
            _ => {}
        }
    }
    types
}

fn member_types<'a>(member: &'a MemberKind, types: &mut Vec<&'a Type>) {
    match member {
        MemberKind::Field { ty, .. } => types.push(ty),
        MemberKind::Method(function) => function_types(function, types),
        MemberKind::Init { .. } => {}
    }
}

fn function_types<'a>(function: &'a FunctionDecl, types: &mut Vec<&'a Type>) {
    for param in &function.params {
        if let Param::Named(named) = param {
            types.push(&named.ty);
        }
    }
    types.extend(&function.ret);
    if let Some(body) = &function.body {
        block_types(body, types);
    }
}

fn block_types<'a>(block: &'a Block, types: &mut Vec<&'a Type>) {
    for stmt in &block.stmts {
        match &stmt.kind {
            StmtKind::Let { ty, .. } => types.extend(ty),
            StmtKind::If {
                branches,
                else_block,
            } => {
                for (_, body) in branches {
                    block_types(body, types);
                }
                if let Some(body) = else_block {
                    block_types(body, types);
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => block_types(body, types),
            StmtKind::Match { arms, .. } => {
                for arm in arms {
                    block_types(&arm.body, types);
                }
            }
            StmtKind::Attempt { body, handlers } => {
                block_types(body, types);
                for handler in handlers {
                    block_types(&handler.body, types);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn substitute_function(function: &mut FunctionDecl, arguments: &HashMap<Name, Type>) {
    for param in &mut function.params {
        if let Param::Named(named) = param {
            substitute_type(&mut named.ty, arguments);
        }
    }
    if let Some(ret) = &mut function.ret {
        substitute_type(ret, arguments);
    }
    if let Some(body) = &mut function.body {
        substitute_block(body, arguments);
    }
}

fn substitute_block(block: &mut Block, arguments: &HashMap<Name, Type>) {
    for stmt in &mut block.stmts {
        match &mut stmt.kind {
            StmtKind::Let { ty: Some(ty), .. } => substitute_type(ty, arguments),
            StmtKind::If {
                branches,
                else_block,
            } => {
                for (_, body) in branches {
                    substitute_block(body, arguments);
                }
                if let Some(body) = else_block {
                    substitute_block(body, arguments);
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                substitute_block(body, arguments);
            }
            StmtKind::Match { arms, .. } => {
                for arm in arms {
                    substitute_block(&mut arm.body, arguments);
                }
            }
            StmtKind::Attempt { body, handlers } => {
                substitute_block(body, arguments);
                for handler in handlers {
                    substitute_block(&mut handler.body, arguments);
                }
            }
            _ => {}
        }
    }
}

/// Replaces each type parameter named in `ty` with the argument written for it.
pub(crate) fn substitute_type(ty: &mut Type, arguments: &HashMap<Name, Type>) {
    match &mut ty.kind {
        TypeKind::Named { path, args } => {
            if args.is_empty()
                && let [segment] = path.segments.as_slice()
                && let Some(argument) = arguments.get(&segment.name)
            {
                *ty = argument.clone();
                return;
            }
            for argument in args {
                substitute_type(argument, arguments);
            }
        }
        TypeKind::Array(element) | TypeKind::Any(element) | TypeKind::Some(element) => {
            substitute_type(element, arguments);
        }
        TypeKind::Tuple(elements) => {
            for element in elements {
                substitute_type(element, arguments);
            }
        }
        TypeKind::Function { params, ret } => {
            for param in params {
                substitute_type(param, arguments);
            }
            substitute_type(ret, arguments);
        }
    }
}
