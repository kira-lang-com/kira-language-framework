use super::*;

pub(crate) fn merge_units(session: &Session, modules: Vec<Module>) -> Result<Module, DriveError> {
    let name = modules
        .last()
        .ok_or(DriveError::Mismatch(DefinitionPhase::Units))?
        .name
        .clone();
    let mut units = modules.into_iter();
    let first = units
        .next()
        .ok_or(DriveError::Mismatch(DefinitionPhase::Units))?;
    let mut merged = Merged::new(first);
    for module in units {
        merged.add(session, module)?;
    }
    merged.module.name = name;
    crate::evaluation::fold_deferred(&mut merged.module).map_err(|message| DriveError::Phase {
        phase: DefinitionPhase::Lower,
        message,
    })?;
    Ok(merged.module)
}

/// The program being built from the units, with where each function, type
/// and global of it sits.
struct Merged {
    module: Module,
    functions: HashMap<Name, usize>,
    types: HashMap<Name, usize>,
    globals: HashMap<Name, usize>,
    // The unit each function was taken from, for a conflict to name.
    origins: HashMap<Name, Name>,
}

impl Merged {
    fn new(module: Module) -> Self {
        let functions = positions(module.functions.iter().map(|function| &function.name));
        let origins = module
            .functions
            .iter()
            .map(|function| (function.name.clone(), module.name.clone()))
            .collect();
        let types = positions(module.types.iter().map(|declaration| &declaration.name));
        let globals = positions(module.globals.iter().map(|global| &global.name));
        Self {
            module,
            functions,
            types,
            globals,
            origins,
        }
    }

    /// Merges one unit's KIR into the program.
    ///
    /// A unit that only declares a function emits a body-less declaration,
    /// so a name may appear in several units; the one definition of it wins.
    /// Two definitions of the same name are the error.
    fn add(&mut self, session: &Session, extra: Module) -> Result<(), DriveError> {
        let base = &mut self.module;
        let unit = extra.name.clone();
        for function in extra.functions {
            let Some(&index) = self.functions.get(&function.name) else {
                self.functions
                    .insert(function.name.clone(), base.functions.len());
                self.origins.insert(function.name.clone(), unit.clone());
                base.functions.push(function);
                continue;
            };
            if base.functions[index].foreign {
                // A unit that does not reach a function declares it by name
                // alone; any other unit's form of it says more.
                if !function.foreign || by_name(&base.functions[index]) {
                    self.origins.insert(function.name.clone(), unit.clone());
                    base.functions[index] = function;
                }
            } else if !function.foreign
                && !same_function_definition(&base.functions[index], &function)
            {
                // A unit gives a function it lowers but never reaches an
                // empty body without a signature; the unit that reaches it
                // defines it.
                if unreached_stub(&function) {
                    continue;
                }
                if unreached_stub(&base.functions[index]) {
                    self.origins.insert(function.name.clone(), unit.clone());
                    base.functions[index] = function;
                    continue;
                }
                let first = self.origins.get(&function.name).map_or("", Name::as_str);
                return Err(DriveError::Conflict(
                    crate::unit_conflict::function_conflict(
                        session,
                        diagnostic_function_name(function.name.as_str()),
                        &crate::unit_conflict::Holder {
                            unit: first,
                            function: &base.functions[index],
                        },
                        &crate::unit_conflict::Holder {
                            unit: unit.as_str(),
                            function: &function,
                        },
                    ),
                ));
            }
        }
        for declaration in extra.types {
            let Some(&index) = self.types.get(&declaration.name) else {
                self.types
                    .insert(declaration.name.clone(), base.types.len());
                base.types.push(declaration);
                continue;
            };
            // The same type declared by two units is one type, as long as the
            // units agree on what it is. A unit that only names a struct
            // agrees with the unit that completes it.
            if declaration.is_incomplete() {
                continue;
            }
            if base.types[index].is_incomplete() {
                base.types[index] = declaration;
                continue;
            }
            if !base.types[index].describes_same(&declaration) {
                if std::env::var_os("KLF_DEBUG_MERGE").is_some() {
                    let cases = |declaration: &klf_kir::TypeDeclaration| -> Vec<String> {
                        declaration
                            .variants
                            .iter()
                            .map(|variant| {
                                format!("{}/{:?}", variant.name.as_str(), variant.payload)
                            })
                            .collect()
                    };
                    eprintln!(
                        "KLF_DEBUG_MERGE {}: unit `{}` cases {:?} against earlier cases {:?} | kind {:?}/{:?} generics {:?}/{:?} fields {:?}/{:?} layout {:?}/{:?}",
                        declaration.name.as_str(),
                        extra.name,
                        cases(&declaration),
                        cases(&base.types[index]),
                        declaration.kind,
                        base.types[index].kind,
                        declaration.generics,
                        base.types[index].generics,
                        declaration.fields.len(),
                        base.types[index].fields.len(),
                        declaration.layout,
                        base.types[index].layout,
                    );
                }
                return Err(DriveError::Conflict(crate::unit_conflict::type_conflict(
                    session,
                    &base.types[index],
                    &declaration,
                )));
            }
        }
        for global in extra.globals {
            let Some(&index) = self.globals.get(&global.name) else {
                self.globals.insert(global.name.clone(), base.globals.len());
                base.globals.push(global);
                continue;
            };
            // A C unit that only declares a global (`extern T x;`, or a
            // tentative `T x;`) holds zeroed storage for it; the unit that
            // defines it with an initializer gives the program its storage.
            if global.init == klf_kir::GlobalInit::Zero
                && base.globals[index].init != klf_kir::GlobalInit::Zero
            {
                continue;
            }
            if base.globals[index].init == klf_kir::GlobalInit::Zero
                && global.init != klf_kir::GlobalInit::Zero
            {
                base.globals[index] = global;
                continue;
            }
            if global.init == klf_kir::GlobalInit::Zero
                && base.globals[index].init == klf_kir::GlobalInit::Zero
                && global.ty != base.globals[index].ty
            {
                // A C declaration of an array of unknown bound names its
                // element; the definition gives the bound.
                if declared_storage(&global.ty) > declared_storage(&base.globals[index].ty) {
                    base.globals[index] = global;
                }
                continue;
            }
            if base.globals[index] != global {
                return Err(DriveError::Conflict(format!(
                    "defines global `{}` in more than one unit",
                    crate::unit_conflict::source_name(global.name.as_str())
                )));
            }
        }
        // Each unit records what it links, imports and witnesses itself.
        extend_distinct_hashed(&mut base.attributes, extra.attributes);
        // Two units that build the same instance record its witnesses alike.
        extend_distinct(&mut base.witnesses, extra.witnesses);
        extend_distinct(&mut base.vtables, extra.vtables);
        extend_distinct(&mut base.traits, extra.traits);
        extend_distinct(&mut base.conformances, extra.conformances);
        extend_distinct(&mut base.imports, extra.imports);
        extend_distinct(&mut base.exports, extra.exports);
        Ok(())
    }
}

/// How many elements of storage a global's type declares: an array's bound,
/// one for anything else.
fn declared_storage(ty: &klf_kir::Type) -> u64 {
    match ty {
        klf_kir::Type::Array(count, _) => *count,
        _ => 1,
    }
}

fn positions<'a>(names: impl Iterator<Item = &'a Name>) -> HashMap<Name, usize> {
    names
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect()
}

/// A declaration without a signature: what a unit declares of a function it
/// does not reach.
fn by_name(function: &Function) -> bool {
    function.parameters.is_empty()
        && function.result == klf_kir::Type::Unit
        && function.c_prototype.is_none()
}

/// The empty body a unit gives a function nothing in it reaches: no
/// signature, and one block that only returns.
fn unreached_stub(function: &Function) -> bool {
    let [block] = function.blocks.as_slice() else {
        return false;
    };
    let returns = |instruction: &klf_kir::Instruction| {
        matches!(instruction.operation, klf_kir::Operation::Return(None))
    };
    by_name(function)
        && block.parameters.is_empty()
        && matches!(block.instructions.as_slice(), [only] if returns(only))
}

fn extend_distinct<T: PartialEq>(base: &mut Vec<T>, extra: Vec<T>) {
    for item in extra {
        if !base.contains(&item) {
            base.push(item);
        }
    }
}

/// [`extend_distinct`] for attributes: units of one program share most of
/// theirs and carry thousands, so the ones already there are looked up by
/// hash.
fn extend_distinct_hashed<T: Eq + std::hash::Hash + Clone>(base: &mut Vec<T>, extra: Vec<T>) {
    let mut seen: std::collections::HashSet<T> = base.iter().cloned().collect();
    for item in extra {
        if seen.insert(item.clone()) {
            base.push(item);
        }
    }
}

fn diagnostic_function_name(name: &str) -> &str {
    let Some((base, suffix)) = name.rsplit_once('#') else {
        return name;
    };
    if !base.is_empty() && !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        base
    } else {
        name
    }
}

fn same_function_definition(left: &Function, right: &Function) -> bool {
    left.name == right.name
        && left.linkage == right.linkage
        && left.abi == right.abi
        && left.nounwind == right.nounwind
        && left.foreign == right.foreign
        && left.generics == right.generics
        && left.attributes == right.attributes
        && same_parameters(&left.parameters, &right.parameters)
        && left.result == right.result
        && left.failure == right.failure
        && left.engine == right.engine
        && same_blocks(&left.blocks, &right.blocks)
}

fn same_blocks(left: &[klf_kir::Block], right: &[klf_kir::Block]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut anonymous = Vec::new();
    left.iter().zip(right).all(|(left, right)| {
        left.name == right.name
            && same_parameters(&left.parameters, &right.parameters)
            && same_instructions(&left.instructions, &right.instructions, &mut anonymous)
    })
}

fn same_instructions(
    left: &[klf_kir::Instruction],
    right: &[klf_kir::Instruction],
    anonymous: &mut Vec<(String, String)>,
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.results == right.results
                && same_operation(&left.operation, &right.operation, anonymous)
        })
}

pub(crate) fn same_operation(
    left: &klf_kir::Operation,
    right: &klf_kir::Operation,
    anonymous: &mut Vec<(String, String)>,
) -> bool {
    match (left, right) {
        (
            klf_kir::Operation::GlobalAddress {
                global: left,
                thread_local: left_tls,
            },
            klf_kir::Operation::GlobalAddress {
                global: right,
                thread_local: right_tls,
            },
        ) if left.as_str().starts_with("anon.") && right.as_str().starts_with("anon.") => {
            if left_tls != right_tls {
                return false;
            }
            if let Some((_, mapped)) = anonymous.iter().find(|(name, _)| name == left.as_str()) {
                return mapped == right.as_str();
            }
            anonymous.push((left.to_string(), right.to_string()));
            true
        }
        _ => left == right,
    }
}

pub(crate) fn same_parameters(left: &[klf_kir::Parameter], right: &[klf_kir::Parameter]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.convention == right.convention
                && left.context == right.context
                && left.ty == right.ty
        })
}
