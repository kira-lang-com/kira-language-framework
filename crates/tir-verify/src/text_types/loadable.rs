use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;
use tir_layout::LayoutEngine;
use tir_types::Type;

use crate::gpu_profile::policy::TypePolicies;

const V8: Code = Code::new(Dialect::Tir, Area::Verifier, 8);

pub(super) fn verify(
    body: &FunctionBody,
    value_types: &HashMap<String, Type>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for block in &body.blocks {
        for parameter in &block.parameters {
            verify_type(
                &parameter.name,
                &parameter.ty,
                parameter.span,
                policies,
                layouts,
                &mut diagnostics,
            );
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                let Some(ty) = value_types.get(result) else {
                    continue;
                };
                verify_type(
                    result,
                    ty,
                    instruction.span,
                    policies,
                    layouts,
                    &mut diagnostics,
                );
            }
        }
    }
    diagnostics
}

fn verify_type(
    name: &str,
    ty: &Type,
    span: tir_diag::Span,
    policies: &TypePolicies,
    layouts: &mut LayoutEngine,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // `*T` is an SSA address handle, even when its pointee `T` is address-only.
    // The pointee's value still must not appear as an SSA result/parameter.
    if !matches!(ty, Type::Address { .. }) && !policies.is_loadable(ty, layouts) {
        diagnostics.push(
            Diagnostic::error(V8, format!("SSA value %{name} has address-only type {ty}"))
                .with_primary_span(span),
        );
    }
}
