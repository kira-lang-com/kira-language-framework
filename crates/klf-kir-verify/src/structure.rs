use klf_diag::Diagnostic;
use klf_kir::{Function, Type};

pub(crate) fn verify_entry_parameters(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    let entry = &function.blocks[0];
    let context_parameters: Vec<_> = function
        .parameters
        .iter()
        .filter(|parameter| parameter.context)
        .collect();
    if context_parameters.len() > 1 {
        diagnostics.push(
            Diagnostic::error("KLF-KIR-1001", "function has multiple context parameters")
                .with_label(function.span, "at most one context parameter is allowed"),
        );
    }
    for parameter in context_parameters {
        if !matches!(
            &parameter.ty,
            Type::Pointer | Type::Address(_) | Type::Reference(_) | Type::Unique(_) | Type::Weak(_)
        ) {
            diagnostics.push(
                Diagnostic::error(
                    "KLF-KIR-1001",
                    "function context parameter must have pointer type",
                )
                .with_label(parameter.span, "context parameters are pointers"),
            );
        }
    }
    if function.parameters.len() != entry.parameters.len() {
        diagnostics.push(
            Diagnostic::error(
                "KLF-KIR-1001",
                format!(
                    "entry block `{}` has {} parameters but function `@{}` has {}",
                    entry.name,
                    entry.parameters.len(),
                    function.name,
                    function.parameters.len()
                ),
            )
            .with_label(
                entry.span,
                "entry parameters must match the function signature",
            ),
        );
        return;
    }
    for (signature, block) in function.parameters.iter().zip(&entry.parameters) {
        if signature.name != block.name
            || signature.ty != block.ty
            || signature.convention != block.convention
            || signature.context != block.context
        {
            diagnostics.push(
                Diagnostic::error(
                    "KLF-KIR-1001",
                    "entry block parameter does not match the function signature",
                )
                .with_label(
                    block.span,
                    format!("expected parameter `%{}`: {}", signature.name, signature.ty),
                ),
            );
        }
    }
}

pub(crate) fn verify_terminators(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    for block in &function.blocks {
        let terminators: Vec<_> = block
            .instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| instruction.operation.is_terminator())
            .collect();
        if terminators.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "KLF-KIR-1003",
                    format!("block `{}` has no terminator", block.name),
                )
                .with_label(
                    block.span,
                    "add exactly one terminator at the end of this block",
                ),
            );
        } else if terminators.len() > 1 {
            for (_, instruction) in terminators.iter().skip(1) {
                diagnostics.push(
                    Diagnostic::error(
                        "KLF-KIR-1003",
                        format!("block `{}` has more than one terminator", block.name),
                    )
                    .with_label(instruction.span, "extra terminator"),
                );
            }
        }
        if let Some((position, instruction)) = terminators.first()
            && *position + 1 != block.instructions.len()
        {
            diagnostics.push(
                Diagnostic::error(
                    "KLF-KIR-1003",
                    format!("terminator in block `{}` is not last", block.name),
                )
                .with_label(instruction.span, "instructions cannot follow a terminator"),
            );
        }
    }
}
