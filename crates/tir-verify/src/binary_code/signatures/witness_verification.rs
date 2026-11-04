use tir_diag::Diagnostic;
use tir_format::{BinaryGenericTable, BinarySignatureTable, BinaryTypeTable};

use super::V2;
use super::generic_types::infer_type_arguments;

pub(super) fn verify(
    generics: &BinaryGenericTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for conformance in &generics.conformances {
        let Some(trait_definition) = generics
            .traits
            .iter()
            .find(|definition| definition.name == conformance.trait_path)
        else {
            continue;
        };
        let mut inferred_arguments = vec![None; trait_definition.generic_parameters.len()];
        for witness in &conformance.witnesses {
            let Some(requirement) = trait_definition
                .methods
                .iter()
                .find(|method| method.name == witness.method)
            else {
                continue;
            };
            let Some(actual) = signatures
                .records
                .get(usize::try_from(witness.function_index).unwrap_or(usize::MAX))
            else {
                continue;
            };
            let mut candidate_arguments = inferred_arguments.clone();
            if !infer_type_arguments(
                types,
                requirement.function_type,
                actual.function_type,
                &mut candidate_arguments,
            ) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "witness function @{} does not match trait requirement {}.{} after generic substitution (required function type index {}, witness function type index {})",
                        actual.name,
                        trait_definition.name,
                        witness.method,
                        requirement.function_type,
                        actual.function_type
                    ),
                ));
            } else {
                inferred_arguments = candidate_arguments;
            }
        }
    }
    diagnostics
}
