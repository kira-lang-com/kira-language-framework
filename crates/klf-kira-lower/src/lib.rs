//! The KLF Kira frontend's checking and lowering to KIR for the M4 subset.
//!
//! Resolution and type checking are folded into lowering here (D-122): the
//! subset — top-level functions over `Int`/sized integers/`Bool`/`Void`, with
//! arithmetic, comparisons, bindings, `if`, `while`, calls and `return` — is
//! small enough that a single tree walk resolves names, checks types and emits
//! KIR. A separate `klf-kira-sema` pass is split out when the language grows.

mod builtins;
mod cli;
mod constants;
mod definition_diagnostics;
mod definition_editor;
mod definition_entrypoints;
mod definition_file_modes;
mod definition_flags;
mod definition_metadata;
mod definition_shader;
mod enums;
mod generics;
mod lower;
mod methods;
mod session_types;
mod signatures;
mod structs;
pub mod testing;
mod ty;
mod wrappers;

use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::Module;
use klf_kira_syntax::ast::{CMember, DeclKind, File, FunctionDecl, MemberKind};
use klf_kira_syntax::{Parsed, parse};
use klf_services::{
    DefaultConvention, Definition, DefinitionArtifact, DefinitionPackage, DefinitionUnit,
    DiagnosticSpec, Exclusivity, LanguageDescriptor, MemoryModel, ModuleBuilder, QueryPhase,
    Requirement, Session, TraitId,
};
use klf_target::Target;
use session_types::TypeHandles;
pub(crate) use signatures::Signature;
use signatures::is_definition_phase;
use std::collections::HashMap;
use ty::FeType;

pub use cli::COMMAND_ENTRY;
pub use definition_metadata::{
    DefinitionEntrypoints, LanguageDefinitionMetadata, extract_language_descriptor,
    extract_language_metadata,
};

/// The KLF Kira frontend as a kernel [`Definition`].
pub struct KiraDefinition;

impl Definition for KiraDefinition {
    fn name(&self) -> String {
        "klfkira".to_owned()
    }

    fn extensions(&self) -> Vec<String> {
        vec!["klfk".to_owned(), "klfd".to_owned()]
    }

    fn language(&self) -> LanguageDescriptor {
        LanguageDescriptor::new(
            "klfkira",
            "klf",
            "1.0.0",
            vec!["klfk".to_owned(), "klfd".to_owned()],
            MemoryModel::Counted,
            Exclusivity::None,
            DefaultConvention::Ridl,
            "std",
        )
        .with_diagnostics(vec![DiagnosticSpec::new(
            2001,
            klf_diag::Severity::Error,
            "runtime diagnostic",
            "a definition reported an error",
        )])
    }

    fn units(
        &self,
        session: &mut Session,
        package: &DefinitionPackage,
    ) -> Option<Vec<DefinitionUnit>> {
        if package.sources().is_empty() {
            session.report(Diagnostic::error(
                "KLF-DEF-0001",
                "definition package has no source files",
            ));
            return None;
        }
        let files = package
            .sources()
            .iter()
            .map(|source| session.add_file(source.name.as_str(), source.text.clone()))
            .collect();
        let unit = DefinitionUnit::from_files(files)?;
        session.register_unit(unit.clone());
        Some(vec![unit])
    }

    fn supports_phase_queries(&self) -> bool {
        true
    }

    fn parse(&self, session: &mut Session, unit: DefinitionUnit) -> Option<DefinitionArtifact> {
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Parse) {
            return Some(artifact);
        }
        let query = session.begin_query(self.name(), &unit, QueryPhase::Parse);
        let result = parse_unit(session, &unit).map(|parsed| {
            let artifact = DefinitionArtifact::new(session, unit.clone(), parsed);
            session.cache_artifact(self.name(), QueryPhase::Parse, &artifact);
            artifact
        });
        session.finish_query(query);
        result
    }

    fn elaborate(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        parsed: &DefinitionArtifact,
    ) -> Option<DefinitionArtifact> {
        if parsed.unit() != unit {
            session.report(Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        if let Some(artifact) = session.cached_artifact(self.name(), &unit, QueryPhase::Elaborate) {
            return Some(artifact);
        }
        let query = session.begin_query(self.name(), &unit, QueryPhase::Elaborate);
        session.record_query_dependency(self.name(), &unit, QueryPhase::Parse);
        let result = parsed
            .downcast_ref::<Parsed>(session)
            .cloned()
            .and_then(|parsed| elaborate_file(session, &parsed.file))
            .map(|elaborated| {
                let artifact = DefinitionArtifact::new(session, unit.clone(), elaborated);
                session.cache_artifact(self.name(), QueryPhase::Elaborate, &artifact);
                artifact
            });
        session.finish_query(query);
        result
    }

    fn lower(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        program: &DefinitionArtifact,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        if program.unit() != unit {
            session.report(Diagnostic::error(
                "KLF-DEF-0003",
                "definition phase artifact belongs to another unit",
            ));
            return None;
        }
        let program = program.downcast_ref::<ElaboratedFile>(session)?.clone();
        let query = session.begin_query(self.name(), &unit, QueryPhase::Lower);
        session.record_query_dependency(self.name(), &unit, QueryPhase::Elaborate);
        let result = lower_elaborated(session, module_name, target, &program);
        session.finish_query(query);
        result
    }

    fn compile(
        &self,
        session: &mut Session,
        source_name: &str,
        source: &str,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        let package = DefinitionPackage::single(source_name, source);
        self.compile_package(session, &package, module_name, target)
    }

    fn compile_unit(
        &self,
        session: &mut Session,
        unit: DefinitionUnit,
        module_name: &str,
        target: Target,
    ) -> Option<Module> {
        compile_file(session, &unit, module_name, target)
    }
}

fn compile_file(
    session: &mut Session,
    unit: &DefinitionUnit,
    module_name: &str,
    target: Target,
) -> Option<Module> {
    let parsed = parse_unit(session, unit)?;
    let elaborated = elaborate_file(session, &parsed.file)?;
    lower_elaborated(session, module_name, target, &elaborated)
}

/// Parses one registered definition unit and reports parser diagnostics into
/// the session.
#[must_use]
pub fn parse_unit(session: &mut Session, unit: &DefinitionUnit) -> Option<Parsed> {
    let mut file = File {
        imports: Vec::new(),
        decls: Vec::new(),
    };
    let mut diagnostics = Vec::new();
    for file_id in unit.files() {
        let parsed = parse(session.file(*file_id));
        diagnostics.extend(parsed.diagnostics.iter().cloned());
        file.imports.extend(parsed.file.imports);
        file.decls.extend(parsed.file.decls);
    }
    for diagnostic in &diagnostics {
        session.report(diagnostic.clone());
    }
    if session.error_count() == 0 {
        Some(Parsed { file, diagnostics })
    } else {
        None
    }
}

/// The KLF Kira elaboration result for one parsed source file.
#[derive(Clone)]
pub struct ElaboratedFile {
    file: File,
    structs: structs::StructTable,
    enums: enums::EnumTable,
    methods: Vec<methods::Method>,
    constants: constants::ConstantTable,
    signatures: HashMap<Name, Signature>,
    types: TypeHandles,
}

impl ElaboratedFile {
    #[must_use]
    pub fn function_type(&self, name: &str) -> Option<klf_services::TypeId> {
        self.types.function_type(&Name::new(name))
    }
}

/// Resolves declarations and function signatures without emitting KIR.
#[must_use]
pub fn elaborate_file(session: &mut Session, file: &File) -> Option<ElaboratedFile> {
    let file = cli::prepare(session, file.clone());
    let file = testing::prepare(session, file);
    let file = lower::project_hook::prepare(file);
    let (file, wrapped) = wrappers::desugar(session, file);
    let file = &file;
    validate_imports(session, file);
    validate_namespace(session, file);
    let traits = collect_traits(session, file);
    let struct_names = structs::names(file);
    let templates = generics::templates(file);
    let mut enum_names = enums::names(file);
    let instances = generics::instantiate(session, file, &templates, &struct_names, &enum_names);
    enum_names.extend(instances.enums.keys().cloned());
    let mut structs = structs::collect(session, file, &enum_names, &wrapped);
    let enum_table = enums::collect(session, file, &struct_names, &enum_names, instances.enums);
    structs.extend(enums::payload_records(&enum_table));
    let mut methods = methods::collect(session, file, &struct_names, &enum_names);
    methods.extend(instances.methods);
    let constants = constants::collect(session, file, &struct_names, &enum_names);
    let names = signatures::TypeNames {
        structs: &struct_names,
        enums: &enum_names,
        traits: &traits,
    };
    let signatures = signatures::collect(session, file, &methods, &names);
    if session.error_count() != 0 {
        return None;
    }
    let types = TypeHandles::build(session, &structs, &enum_table, &signatures);
    session_types::register_conformances(session, file, &types, &traits, &structs, &enum_table);
    if session.error_count() != 0 {
        return None;
    }
    Some(ElaboratedFile {
        file: file.clone(),
        structs,
        enums: enum_table,
        methods,
        constants,
        signatures,
        types,
    })
}

fn validate_imports(session: &mut Session, file: &File) {
    for import in &file.imports {
        let path = import
            .path
            .segments
            .iter()
            .map(|segment| segment.name.as_str())
            .collect::<Vec<_>>()
            .join(".");
        if path != "KLF" && path != testing::MODULE {
            session.report(
                Diagnostic::error(
                    "KLFK-E0206",
                    format!("import `{path}` is outside the current definition subset"),
                )
                .with_label(import.span, "not supported yet"),
            );
        }
    }
}

fn validate_namespace(session: &mut Session, file: &File) {
    let mut declarations = HashMap::new();
    for decl in &file.decls {
        let Some((name, span)) = declaration_name(&decl.kind) else {
            continue;
        };
        if let Some(previous) = declarations.get(name) {
            session.report(
                Diagnostic::error_at(
                    "KLFK-E0200",
                    span,
                    format!("duplicate declaration `{name}`"),
                )
                .with_note_at(*previous, "first declaration is here"),
            );
        } else {
            declarations.insert(name.clone(), span);
        }
    }
}

fn declaration_name(kind: &DeclKind) -> Option<(&Name, klf_source::Span)> {
    match kind {
        DeclKind::Function(function) => Some((&function.name.name, function.name.span)),
        DeclKind::Struct(structure) => Some((&structure.name.name, structure.name.span)),
        DeclKind::Enum(enum_decl) => Some((&enum_decl.name.name, enum_decl.name.span)),
        DeclKind::Trait(trait_decl) => Some((&trait_decl.name.name, trait_decl.name.span)),
        DeclKind::Construct(construct) => Some((&construct.name.name, construct.name.span)),
        DeclKind::Alias(alias) => Some((&alias.name.name, alias.name.span)),
        DeclKind::Distinct(distinct) => Some((&distinct.name.name, distinct.name.span)),
        DeclKind::Constant(constant) => Some((&constant.name.name, constant.name.span)),
        DeclKind::Extend(_) => None,
    }
}

fn collect_traits(session: &mut Session, file: &File) -> HashMap<Name, TraitId> {
    let mut traits = HashMap::new();
    for decl in &file.decls {
        let DeclKind::Trait(trait_decl) = &decl.kind else {
            continue;
        };
        if !trait_decl.generics.is_empty() {
            session.report(
                Diagnostic::error(
                    "KLFK-E0206",
                    "generic traits are outside the current subset",
                )
                .with_label(trait_decl.span, "remove the generic parameters"),
            );
        }
        let requirements = trait_decl
            .items
            .iter()
            .map(|item| match item {
                klf_kira_syntax::ast::TraitItem::AssocType { name, .. } => Requirement {
                    name: name.name.clone(),
                },
                klf_kira_syntax::ast::TraitItem::Method(function) => Requirement {
                    name: function.name.name.clone(),
                },
            })
            .collect();
        let trait_id =
            session.declare_trait(trait_decl.name.name.clone(), requirements, Vec::new());
        traits.insert(trait_decl.name.name.clone(), trait_id);
    }
    for decl in &file.decls {
        let DeclKind::Trait(trait_decl) = &decl.kind else {
            continue;
        };
        let Some(trait_id) = traits.get(&trait_decl.name.name).copied() else {
            continue;
        };
        let parents = trait_decl
            .conforms
            .iter()
            .filter_map(|parent| trait_id_for(session, parent, &traits))
            .collect();
        session.complete_trait(trait_id, parents);
    }
    traits
}

pub(crate) fn trait_id_for(
    session: &mut Session,
    ty: &klf_kira_syntax::ast::Type,
    traits: &HashMap<Name, TraitId>,
) -> Option<TraitId> {
    let klf_kira_syntax::ast::TypeKind::Named { path, args } = &ty.kind else {
        session.report(
            Diagnostic::error("KLFK-E0206", "trait references must name a declared trait")
                .with_label(ty.span, "expected a trait name"),
        );
        return None;
    };
    if !args.is_empty() || path.segments.len() != 1 {
        session.report(
            Diagnostic::error(
                "KLFK-E0206",
                "generic and qualified trait references are outside the current subset",
            )
            .with_label(ty.span, "expected one trait name"),
        );
        return None;
    }
    let name = &path.segments[0].name;
    let Some(trait_id) = traits.get(name).copied() else {
        session.report(
            Diagnostic::error("KLFK-E0200", format!("unknown trait `{name}`"))
                .with_label(ty.span, "not declared in this package"),
        );
        return None;
    };
    Some(trait_id)
}

/// Lowers an elaborated KLF Kira file to a raw KIR module.
#[must_use]
pub fn lower_elaborated(
    session: &mut Session,
    module_name: &str,
    target: Target,
    elaborated: &ElaboratedFile,
) -> Option<Module> {
    let mut module = ModuleBuilder::new(Name::new(module_name), "klfkira", target);
    let counts = std::cell::RefCell::new(lower::ownership::NeededFunctions::default());
    for declaration in structs::type_declarations(&elaborated.structs) {
        module.add_type(declaration);
    }
    for declaration in enums::type_declarations(&elaborated.enums) {
        module.add_type(declaration);
    }
    for declaration in session_types::witness_declarations(session) {
        module.add_witness(declaration);
    }
    for declaration in builtins::foreign_declarations(session) {
        module.add_function(declaration);
    }
    for method in &elaborated.methods {
        add_lowered_function(
            session,
            &mut module,
            elaborated,
            &counts,
            &method.decl,
            Some(&method.owner),
        );
    }
    for decl in &elaborated.file.decls {
        match &decl.kind {
            DeclKind::Function(func) => {
                add_lowered_function(session, &mut module, elaborated, &counts, func, None);
            }
            DeclKind::Construct(construct) => {
                for member in &construct.members {
                    if let CMember::Member(member) = member
                        && let MemberKind::Method(function) = &member.kind
                    {
                        add_construct_method(session, &mut module, elaborated, &counts, function);
                    }
                }
            }
            DeclKind::Struct(_)
            | DeclKind::Enum(_)
            | DeclKind::Trait(_)
            | DeclKind::Extend(_)
            | DeclKind::Constant(_) => {}
            _ => {
                session.report(
                    Diagnostic::error("KLFK-E0206", "this declaration is outside the M4 subset")
                        .with_label(decl.span, "only functions, structs and enums are lowered"),
                );
            }
        }
    }
    add_instances(session, &mut module, elaborated, &counts);
    if session.error_count() == 0 {
        lower::ownership::add_count_functions(session, elaborated, &counts, &mut module);
        let mut counts = counts.into_inner();
        for function in std::mem::take(&mut counts.closures) {
            module.add_function(function);
        }
        for declaration in lower::closures::context_declarations(&counts.contexts) {
            module.add_type(declaration);
        }
        Some(module.finish())
    } else {
        None
    }
}

/// Lowers a method a `Language` construct declares: a definition phase as
/// it is written, and each entry the driver calls as the shim it needs.
fn add_construct_method(
    session: &mut Session,
    module: &mut ModuleBuilder,
    elaborated: &ElaboratedFile,
    counts: &std::cell::RefCell<lower::ownership::NeededFunctions>,
    function: &FunctionDecl,
) {
    if is_definition_phase(function) {
        add_lowered_function(session, module, elaborated, counts, function, None);
    }
    if function.body.is_none() {
        return;
    }
    let entry = match function.name.name.as_str() {
        lower::compat_driver::COMPAT_DRIVER => lower::compat_driver::lower_entry,
        lower::run_program::RUN_PROGRAM => lower::run_program::lower_entry,
        lower::shader_emit::EMIT => lower::shader_emit::lower_entry,
        lower::format_hook::FORMAT => lower::format_hook::lower_entry,
        lower::project_hook::PROJECT => lower::project_hook::lower_entry,
        _ => return,
    };
    module.add_function(entry(session, elaborated, counts, function.span));
}

/// Lowers a function that takes no type parameters. A generic one is
/// lowered only as the instances its calls ask for.
fn add_lowered_function(
    session: &mut Session,
    module: &mut ModuleBuilder,
    elaborated: &ElaboratedFile,
    counts: &std::cell::RefCell<lower::ownership::NeededFunctions>,
    function: &FunctionDecl,
    owner: Option<&FeType>,
) {
    if !function.generics.is_empty() {
        return;
    }
    if let Some(function) =
        lower::lower_function(session, elaborated, counts, function, owner, None)
    {
        module.add_function(function);
    }
}

/// Lowers every instance of a generic function the module's code calls,
/// and every instance those instances call in turn.
fn add_instances(
    session: &mut Session,
    module: &mut ModuleBuilder,
    elaborated: &ElaboratedFile,
    counts: &std::cell::RefCell<lower::ownership::NeededFunctions>,
) {
    let mut generic: HashMap<&Name, (&FunctionDecl, Option<&FeType>)> = HashMap::new();
    for decl in &elaborated.file.decls {
        if let DeclKind::Function(function) = &decl.kind
            && !function.generics.is_empty()
        {
            generic.insert(&function.name.name, (function, None));
        }
    }
    for method in &elaborated.methods {
        if !method.decl.generics.is_empty() {
            generic.insert(&method.decl.name.name, (&method.decl, Some(&method.owner)));
        }
    }
    let mut lowered = 0;
    loop {
        let Some(instance) = counts.borrow().instances.get(lowered).cloned() else {
            return;
        };
        lowered += 1;
        let Some((function, owner)) = generic.get(&instance.base).copied() else {
            continue;
        };
        if let Some(function) = lower::lower_function(
            session,
            elaborated,
            counts,
            function,
            owner,
            Some(&instance),
        ) {
            module.add_function(function);
        }
    }
}

/// Checks and lowers a parsed KLF Kira file to a raw KIR module. Returns the
/// module when no errors were reported; diagnostics land in `session`.
#[must_use]
pub fn lower_file(
    session: &mut Session,
    module_name: &str,
    target: Target,
    file: &File,
) -> Option<Module> {
    let elaborated = elaborate_file(session, file)?;
    lower_elaborated(session, module_name, target, &elaborated)
}

pub(crate) fn resolve_type(
    session: &mut Session,
    ty: &klf_kira_syntax::ast::Type,
) -> Option<FeType> {
    use klf_kira_syntax::ast::TypeKind;
    if let TypeKind::Named { path, args } = &ty.kind
        && args.is_empty()
        && path.segments.len() == 1
        && let Some(fe) = FeType::from_name(path.segments[0].name.as_str())
    {
        return Some(fe);
    }
    session.report(
        Diagnostic::error("KLFK-E0206", "this type is outside the M4 subset")
            .with_label(ty.span, "expected Int, a sized integer, Bool or Void"),
    );
    None
}

#[must_use]
pub(crate) fn type_error(span: klf_source::Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error("KLFK-E0201", message).with_label(span, "here")
}
