use super::*;

pub(super) fn compile_file(
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

/// Parses one registered source through the persistent frontend query store.
///
/// Parser diagnostics are retained with the query and replayed on a cache
/// hit, so IDE and build-service callers can reuse this input across requests.
#[must_use]
pub fn parse_registered_file(session: &mut Session, file: klf_core::FileId) -> Option<Parsed> {
    parse_file_incremental(session, file)
}

/// The KLF Kira elaboration result for one parsed source file.
#[derive(Clone)]
pub struct ElaboratedFile {
    pub(super) file: File,
    pub(super) structs: structs::StructTable,
    pub(super) enums: enums::EnumTable,
    pub(super) methods: Vec<methods::Method>,
    pub(super) constants: constants::ConstantTable,
    pub(super) signatures: HashMap<Name, Arc<Signature>>,
    pub(super) types: TypeHandles,
    pub(super) interface_fingerprint: u64,
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
    let interface_fingerprint = interface_fingerprint(session, file, &signatures);
    Some(ElaboratedFile {
        file: file.clone(),
        structs,
        enums: enum_table,
        methods,
        constants,
        signatures,
        types,
        interface_fingerprint,
    })
}

fn interface_fingerprint(
    session: &Session,
    file: &File,
    signatures: &HashMap<Name, Arc<Signature>>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut signatures: Vec<_> = signatures.iter().collect();
    signatures.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    for (name, signature) in signatures {
        name.hash(&mut hasher);
        signature.generics.hash(&mut hasher);
        signature.params.hash(&mut hasher);
        signature.moves.hash(&mut hasher);
        signature.result.hash(&mut hasher);
        signature.receiver.hash(&mut hasher);
        signature.source_function.hash(&mut hasher);
        if let Some(service) = &signature.service {
            service.name.hash(&mut hasher);
            service.symbol.hash(&mut hasher);
        }
        let mut bounds: Vec<_> = signature.bounds.iter().collect();
        bounds.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        for (generic, traits) in bounds {
            generic.hash(&mut hasher);
            traits.hash(&mut hasher);
        }
    }
    for declaration in &file.decls {
        if !matches!(declaration.kind, DeclKind::Function(_)) {
            hash_source_span(session, declaration.span, &mut hasher);
        }
    }
    for import in &file.imports {
        hash_source_span(session, import.span, &mut hasher);
    }
    hasher.finish()
}

fn hash_source_span(session: &Session, span: klf_source::Span, hasher: &mut impl Hasher) {
    span.file.hash(hasher);
    span.start.hash(hasher);
    span.end.hash(hasher);
    if session.has_file(span.file) {
        let text = session.file(span.file).text();
        let start = usize::try_from(span.start).unwrap_or(usize::MAX);
        let end = usize::try_from(span.end).unwrap_or(usize::MAX);
        text.get(start..end).unwrap_or(text).hash(hasher);
    }
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
    lower_elaborated_impl(session, None, module_name, target, elaborated)
}

pub(super) fn lower_elaborated_cached(
    session: &mut Session,
    unit: &DefinitionUnit,
    module_name: &str,
    target: Target,
    elaborated: &ElaboratedFile,
) -> Option<Module> {
    lower_elaborated_impl(session, Some(unit), module_name, target, elaborated)
}

fn lower_elaborated_impl(
    session: &mut Session,
    cache_unit: Option<&DefinitionUnit>,
    module_name: &str,
    target: Target,
    elaborated: &ElaboratedFile,
) -> Option<Module> {
    let module_context = lower_context(module_name, &target);
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
                if let Some(unit) = cache_unit {
                    add_lowered_function_cached(
                        session,
                        &mut module,
                        elaborated,
                        &counts,
                        func,
                        unit,
                        &module_context,
                    );
                } else {
                    add_lowered_function(session, &mut module, elaborated, &counts, func, None);
                }
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

struct CachedLoweredFunction {
    fingerprint: u64,
    function: Arc<Function>,
}

fn add_lowered_function_cached(
    session: &mut Session,
    module: &mut ModuleBuilder,
    elaborated: &ElaboratedFile,
    counts: &std::cell::RefCell<lower::ownership::NeededFunctions>,
    function: &FunctionDecl,
    unit: &DefinitionUnit,
    module_context: &str,
) {
    if !function.generics.is_empty() {
        return;
    }
    let fingerprint = function_fingerprint(session, elaborated, function);
    let context = format!("{module_context}:function:{}", function.name.name);
    let cached =
        session.cached_artifact_with_context("klfkira", unit, QueryPhase::Lower, context.clone());
    if let Some(value) = cached
        .as_ref()
        .and_then(|artifact| artifact.downcast_ref::<CachedLoweredFunction>(session))
        && value.fingerprint == fingerprint
    {
        module.add_shared_function(Arc::clone(&value.function));
        return;
    }

    let query = session.begin_derived_query_with_context(
        "klfkira",
        unit,
        QueryPhase::Lower,
        context.clone(),
    );
    let before = counts.borrow().generated_dependency_requests;
    let lowered = lower::lower_function(session, elaborated, counts, function, None, None);
    let can_cache = counts.borrow().generated_dependency_requests == before;
    if let Some(lowered) = lowered {
        let lowered = Arc::new(lowered);
        if can_cache {
            let payload = Arc::new(CachedLoweredFunction {
                fingerprint,
                function: Arc::clone(&lowered),
            });
            if let Some(artifact) = &cached {
                session.replace_artifact(artifact.id(), Arc::clone(&payload));
                session.cache_artifact_with_context(
                    "klfkira",
                    QueryPhase::Lower,
                    context,
                    artifact,
                );
            } else {
                let artifact = DefinitionArtifact::new_shared(session, unit.clone(), payload);
                session.cache_artifact_with_context(
                    "klfkira",
                    QueryPhase::Lower,
                    context,
                    &artifact,
                );
            }
            module.add_shared_function(lowered);
        } else {
            module.add_function(Arc::unwrap_or_clone(lowered));
        }
    }
    session.finish_query(query);
}

pub(super) fn lower_context(module_name: &str, target: &Target) -> String {
    format!("{module_name}@{}", target.triple())
}

fn function_fingerprint(
    session: &Session,
    elaborated: &ElaboratedFile,
    function: &FunctionDecl,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    elaborated.interface_fingerprint.hash(&mut hasher);
    function.name.name.hash(&mut hasher);
    hash_source_span(session, function.span, &mut hasher);
    hasher.finish()
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
