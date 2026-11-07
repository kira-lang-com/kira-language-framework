use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{BinaryFunctionBody, BinaryType, BinaryTypeTable};

const V8: Code = Code::new(Dialect::Tir, Area::Verifier, 8);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut value = 0usize;
    for block in &body.blocks {
        for parameter in &block.parameters {
            check_value(
                value,
                parameter.ty,
                None,
                value_types,
                types,
                function_name,
                &mut diagnostics,
            );
            value = value.saturating_add(1);
        }
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for instruction in instructions {
            for _ in 0..instruction.result_count {
                if let Some(ty) = value_types.get(value).copied().flatten() {
                    check_value(
                        value,
                        ty,
                        Some(instruction.offset),
                        value_types,
                        types,
                        function_name,
                        &mut diagnostics,
                    );
                }
                value = value.saturating_add(1);
            }
        }
    }
    diagnostics
}

fn check_value(
    value: usize,
    ty: u64,
    offset: Option<u64>,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if value_types.get(value).copied().flatten() != Some(ty)
        || matches!(
            types.entries.get(ty as usize),
            Some(BinaryType::Address { .. })
        )
        || crate::binary_code::types::aggregate_instructions::is_loadable_type(types, ty)
    {
        return;
    }
    let mut diagnostic = Diagnostic::error(
        V8,
        format!("function @{function_name}: SSA value %{value} has address-only type index {ty}"),
    );
    if let Some(offset) = offset {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        diagnostic = diagnostic.with_primary_span(Span::new(start, start));
    }
    diagnostics.push(diagnostic);
}
