//! Structural and type verification for KIR modules.

mod c_seam;
mod cfg;
mod checks;
mod ownership;
mod signatures;
mod structure;

use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Instruction, Module, Parameter, Stage, Type};
use klf_source::Span;
use rustc_hash::FxHashMap as HashMap;

const V1: &str = "KLF-KIR-1001";
const V2: &str = "KLF-KIR-1002";
const V3: &str = "KLF-KIR-1003";

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    pub(crate) block: usize,
    pub(crate) position: Option<usize>,
    pub(crate) ty: Option<Type>,
    span: Span,
}

/// Verifies every function in a module against the rules permitted by its stage.
///
/// # Panics
/// Panics when a verifier thread panics.
#[must_use]
pub fn verify_module(module: &Module) -> Vec<Diagnostic> {
    verify_module_functions(module, |_| true)
}

/// [`verify_module`] for the functions `chosen` picks, against the whole
/// module: what a build verified before and has not changed since need not
/// be verified again, while what it calls is still read where it now is.
///
/// # Panics
/// Panics when a verifier thread panics.
#[must_use]
pub fn verify_module_functions(
    module: &Module,
    chosen: impl Fn(&Function) -> bool + Sync,
) -> Vec<Diagnostic> {
    // Each function verifies on its own, so the machine's threads take
    // batches of functions in turn and their diagnostics join in module order.
    const BATCH: usize = 32;
    let index = klf_kir::ModuleIndex::of(module);
    let verify_batch = |functions: &[Function]| {
        let mut diagnostics = Vec::new();
        for function in functions {
            if !chosen(function) {
                continue;
            }
            let first = diagnostics.len();
            verify_function(function, module, &index, &mut diagnostics);
            for diagnostic in &mut diagnostics[first..] {
                diagnostic.notes.push(klf_diag::Note {
                    span: None,
                    message: format!("in function `@{}`", function.name),
                });
            }
        }
        diagnostics
    };
    let batches: Vec<&[Function]> = module.functions.chunks(BATCH).collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(batches.len());
    if threads <= 1 {
        return verify_batch(&module.functions);
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut found: Vec<(usize, Vec<Diagnostic>)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut taken = Vec::new();
                    loop {
                        let batch = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(functions) = batches.get(batch) else {
                            break taken;
                        };
                        let diagnostics = verify_batch(functions);
                        if !diagnostics.is_empty() {
                            taken.push((batch, diagnostics));
                        }
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("a verifier thread panicked"))
            .collect()
    });
    found.sort_by_key(|(batch, _)| *batch);
    found
        .into_iter()
        .flat_map(|(_, diagnostics)| diagnostics)
        .collect()
}

fn verify_function(
    function: &Function,
    module: &Module,
    index: &klf_kir::ModuleIndex<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let types = &module.types;
    c_seam::verify_c_prototype(function, module, diagnostics);
    if function.foreign {
        return;
    }
    if function.blocks.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                V3,
                format!("function `@{}` has no entry block", function.name),
            )
            .with_label(
                function.span,
                "a function definition requires a terminated entry block",
            ),
        );
        return;
    }

    structure::verify_entry_parameters(function, diagnostics);
    structure::verify_terminators(function, diagnostics);

    let blocks = collect_blocks(function, diagnostics);
    let mut definitions = collect_definitions(function, diagnostics);
    infer_result_types(function, index, &mut definitions);
    let predecessors = cfg::collect_predecessors(function, &blocks, diagnostics);
    let dominators = cfg::compute_dominators(function.blocks.len(), &predecessors);

    verify_uses(function, &definitions, &dominators, diagnostics);
    checks::verify_types(function, index, &blocks, &definitions, diagnostics);
    signatures::verify_generics(function, diagnostics);
    signatures::verify_asm(function, diagnostics);
    if matches!(module.stage, Stage::Canonical) {
        ownership::verify_ownership(function, &definitions, &blocks, diagnostics);
        ownership::verify_borrows(function, &blocks, diagnostics);
        ownership::verify_accesses(function, &blocks, diagnostics);
        signatures::verify_signature(function, diagnostics);
        signatures::verify_calls(function, index, diagnostics);
        signatures::verify_address_only(function, types, &definitions, diagnostics);
    }
}

fn collect_blocks(function: &Function, diagnostics: &mut Vec<Diagnostic>) -> HashMap<Name, usize> {
    let mut blocks = HashMap::default();
    for (index, block) in function.blocks.iter().enumerate() {
        if blocks.insert(block.name.clone(), index).is_some() {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!("basic block `{}` is defined more than once", block.name),
                )
                .with_label(block.span, "duplicate block definition"),
            );
        }
    }
    blocks
}

fn collect_definitions(
    function: &Function,
    diagnostics: &mut Vec<Diagnostic>,
) -> HashMap<Name, Definition> {
    let mut definitions =
        HashMap::with_capacity_and_hasher(value_count(function), rustc_hash::FxBuildHasher);
    for (block_index, block) in function.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            insert_definition(
                &mut definitions,
                parameter,
                block_index,
                None,
                Some(parameter.ty.clone()),
                diagnostics,
            );
        }
        for (position, instruction) in block.instructions.iter().enumerate() {
            for result in &instruction.results {
                let definition = Definition {
                    block: block_index,
                    position: Some(position),
                    ty: None,
                    span: instruction.span,
                };
                if definitions.insert(result.clone(), definition).is_some() {
                    diagnostics.push(
                        Diagnostic::error(
                            V1,
                            format!("SSA value `%{result}` is defined more than once"),
                        )
                        .with_label(instruction.span, "second definition"),
                    );
                }
            }
        }
    }
    definitions
}

/// How many values `function` defines: its block parameters and every result.
fn value_count(function: &Function) -> usize {
    function
        .blocks
        .iter()
        .map(|block| {
            block.parameters.len()
                + block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.results.len())
                    .sum::<usize>()
        })
        .sum()
}

fn insert_definition(
    definitions: &mut HashMap<Name, Definition>,
    parameter: &Parameter,
    block: usize,
    position: Option<usize>,
    ty: Option<Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let definition = Definition {
        block,
        position,
        ty,
        span: parameter.span,
    };
    if definitions
        .insert(parameter.name.clone(), definition)
        .is_some()
    {
        diagnostics.push(
            Diagnostic::error(
                V1,
                format!("SSA value `%{}` is defined more than once", parameter.name),
            )
            .with_label(parameter.span, "second definition"),
        );
    }
}

fn pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) => Some(inner),
        _ => None,
    }
}

/// Fills in the result types the definition map could not name directly, using
/// the shared inference in `klf-kir` so the verifier and the backends agree on
/// what an instruction produces.
fn infer_result_types(
    function: &Function,
    index: &klf_kir::ModuleIndex<'_>,
    definitions: &mut HashMap<Name, Definition>,
) {
    let inferred = klf_kir::infer_value_types_in(function, index);
    for (name, definition) in definitions.iter_mut() {
        if definition.ty.is_none() {
            definition.ty = inferred.get(name).cloned();
        }
    }
}

fn verify_uses(
    function: &Function,
    definitions: &HashMap<Name, Definition>,
    dominators: &cfg::Dominance,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (position, instruction) in block.instructions.iter().enumerate() {
            for value in instruction.operation.used_values() {
                let Some(definition) = definitions.get(value) else {
                    diagnostics.push(
                        Diagnostic::error(V1, format!("use of undefined SSA value `%{value}`"))
                            .with_label(instruction.span, "undefined value used here"),
                    );
                    continue;
                };
                let dominates = if definition.block == block_index {
                    definition
                        .position
                        .is_none_or(|definition_position| definition_position < position)
                } else {
                    dominators.dominates(definition.block, block_index)
                };
                if !dominates {
                    diagnostics.push(
                        Diagnostic::error(
                            V1,
                            format!("definition of `%{value}` does not dominate this use"),
                        )
                        .with_label(instruction.span, "use is not dominated by its definition")
                        .with_label(definition.span, "value is defined here"),
                    );
                }
            }
        }
    }
}

fn value_type<'a>(name: &Name, definitions: &'a HashMap<Name, Definition>) -> Option<&'a Type> {
    definitions.get(name)?.ty.as_ref()
}

fn type_error(
    instruction: &Instruction,
    message: impl Into<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    diagnostics.push(
        Diagnostic::error(V2, message)
            .with_label(instruction.span, "instruction is not well typed"),
    );
}
