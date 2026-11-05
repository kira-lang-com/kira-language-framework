use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{GpuEntryKind, TextCallingConvention, TextImage, TextItemKind};

mod capabilities;
mod fields;

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub(super) fn verify_entry_records(image: &TextImage) -> Vec<Diagnostic> {
    let mut functions = HashMap::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let (Some(name), Some(signature_name)) = (
            super::gpu::function_symbol(source),
            super::gpu::function_signature_name(source),
        ) else {
            continue;
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|signature| signature.name == signature_name)
        else {
            continue;
        };
        functions.insert(
            name,
            (
                signature.convention,
                item.span,
                super::symbols::function_is_exported(source),
                signature,
            ),
        );
    }

    let mut referenced = HashSet::new();
    let mut diagnostics = Vec::new();
    for entry in &image.entries {
        let Some(item) = image
            .items
            .iter()
            .find(|item| item.kind == TextItemKind::Entry && item.span.start == entry.span.start)
        else {
            continue;
        };
        let source = image.item_text(*item);
        let stage = gpu_entry_kind_name(entry.kind);
        let root = &entry.function;
        diagnostics.extend(fields::verify(source, stage, item.span.start));
        if !referenced.insert(root.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("GPU function @{root} has multiple entry records"),
                )
                .with_primary_span(item.span),
            );
        }
        let Some((convention, function_span, exported, signature)) = functions.get(root).copied()
        else {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("GPU entry record references missing function @{root}"),
                )
                .with_primary_span(item.span),
            );
            continue;
        };
        let expected = if stage == "visible" {
            TextCallingConvention::Gpu
        } else {
            TextCallingConvention::Entry
        };
        if convention != expected {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("GPU {stage} record references @{root} with {convention:?} convention"),
                )
                .with_primary_span(item.span)
                .with_note_at(function_span, "referenced function is declared here"),
            );
        }
        if stage == "visible" && !exported {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("visible GPU function @{root} must be exported"),
                )
                .with_primary_span(item.span)
                .with_note_at(function_span, "referenced function is declared here"),
            );
        }
        if stage != "visible" {
            verify_entry_signature(source, stage, signature, item.span, &mut diagnostics);
        }
        if stage != "visible" {
            let mut required = fields::record_capability_requirements(source, stage);
            let usage = capabilities::analyze(image, root, stage);
            required.extend(usage.capabilities);
            if let Some(declared) = fields::declared_capabilities(source) {
                for capability in required.difference(&declared) {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY22,
                            format!(
                                "GPU {stage} entry @{root} uses undeclared capability '{capability}'"
                            ),
                        )
                        .with_primary_span(item.span),
                    );
                }
                for capability in declared.difference(&required) {
                    if stage == "node" && capability == "work_graphs.mesh" {
                        continue;
                    }
                    diagnostics.push(
                        Diagnostic::error(
                            VFY22,
                            format!(
                                "GPU {stage} entry @{root} declares unused capability '{capability}'"
                            ),
                        )
                        .with_primary_span(item.span),
                    );
                }
            }
            for id in &usage.conflicting_specialization_constants {
                diagnostics.push(
                    Diagnostic::error(
                        VFY22,
                        format!("GPU entry @{root} has conflicting declarations for specialization constant {id}"),
                    )
                    .with_primary_span(item.span),
                );
            }
            if let Some(recorded) = fields::specialization_constants(source) {
                for (id, constant) in &usage.specialization_constants {
                    match recorded.get(id) {
                        None => diagnostics.push(
                            Diagnostic::error(
                                VFY22,
                                format!("GPU entry @{root} omits specialization constant {id}"),
                            )
                            .with_primary_span(item.span),
                        ),
                        Some(recorded)
                            if recorded.ty != constant.ty
                                || recorded.default != constant.default =>
                        {
                            diagnostics.push(
                                Diagnostic::error(
                                    VFY22,
                                    format!("GPU entry @{root} has mismatched type or default for specialization constant {id}"),
                                )
                                .with_primary_span(item.span),
                            );
                        }
                        _ => {}
                    }
                }
                for id in recorded.keys() {
                    if !usage.specialization_constants.contains_key(id) {
                        diagnostics.push(
                            Diagnostic::error(
                                VFY22,
                                format!(
                                    "GPU entry @{root} records unused specialization constant {id}"
                                ),
                            )
                            .with_primary_span(item.span),
                        );
                    }
                }
            }
        }
    }

    for (name, (convention, span, _, _)) in functions {
        if convention == TextCallingConvention::Entry && !referenced.contains(&name) {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("entry function @{name} has no GPU entry record"),
                )
                .with_primary_span(span),
            );
        }
    }
    diagnostics
}

fn verify_entry_signature(
    source: &str,
    stage: &str,
    signature: &tvisa_format::TextSignature,
    span: tir_diag::Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if signature.fails || signature.variadic.is_some() || signature.noreturn {
        diagnostics.push(
            Diagnostic::error(
                VFY22,
                format!("GPU {stage} entry signature cannot be failing, variadic, or noreturn"),
            )
            .with_primary_span(span),
        );
    }

    let input_fields = fields::stage_field_types(source, "stage_input");
    if let Some(input_fields) = input_fields {
        let expected = input_fields.iter().map(|(ty, _)| ty).collect::<Vec<_>>();
        if !expected.is_empty()
            && !flattened_parameter_types(signature)
                .windows(expected.len())
                .any(|window| {
                    window
                        .iter()
                        .zip(&expected)
                        .all(|(actual, expected)| *actual == Some(*expected))
                })
        {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("GPU {stage} entry signature does not contain its stage input fields in order"),
                )
                .with_primary_span(span),
            );
        }
    }

    let output_fields = fields::stage_field_types(source, "stage_output");
    if matches!(stage, "vertex" | "fragment") {
        if let Some(output_fields) = output_fields {
            let expected = output_fields.iter().map(|(ty, _)| ty).collect::<Vec<_>>();
            let actual = flattened_result_types(signature);
            if actual != expected {
                diagnostics.push(
                    Diagnostic::error(
                        VFY22,
                        format!("GPU {stage} entry result fields disagree with its stage outputs"),
                    )
                    .with_primary_span(span),
                );
            } else if let [tvisa_format::ResultItem::Group(group)] = signature.results.as_slice()
                && (group.members.len() != output_fields.len()
                    || group
                        .members
                        .iter()
                        .zip(&output_fields)
                        .any(|(member, (_, offset))| member.offset != *offset))
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY22,
                        format!("GPU {stage} entry result layout disagrees with its stage outputs"),
                    )
                    .with_primary_span(span),
                );
            }
        }
    } else if !flattened_result_types(signature).is_empty() {
        diagnostics.push(
            Diagnostic::error(VFY22, format!("GPU {stage} entry must return unit"))
                .with_primary_span(span),
        );
    }
}

fn flattened_parameter_types(
    signature: &tvisa_format::TextSignature,
) -> Vec<Option<&tir_types::Type>> {
    let mut types = Vec::new();
    for parameter in &signature.parameters {
        match parameter {
            tvisa_format::ParameterItem::Leaf(ty) => types.push(Some(ty)),
            tvisa_format::ParameterItem::Group(group) => {
                types.extend(group.members.iter().map(|member| Some(&member.ty)));
            }
            tvisa_format::ParameterItem::ByRef { .. }
            | tvisa_format::ParameterItem::Sret { .. }
            | tvisa_format::ParameterItem::Context
            | tvisa_format::ParameterItem::CAggregate(_) => types.push(None),
        }
    }
    types
}

fn flattened_result_types(signature: &tvisa_format::TextSignature) -> Vec<&tir_types::Type> {
    let mut types = Vec::new();
    for result in &signature.results {
        match result {
            tvisa_format::ResultItem::Leaf(ty) => types.push(ty),
            tvisa_format::ResultItem::Group(group) => {
                types.extend(group.members.iter().map(|member| &member.ty));
            }
        }
    }
    types
}

pub(super) fn gpu_entry_kind_name(kind: GpuEntryKind) -> &'static str {
    match kind {
        GpuEntryKind::Compute => "compute",
        GpuEntryKind::Object => "object",
        GpuEntryKind::Mesh => "mesh",
        GpuEntryKind::Vertex => "vertex",
        GpuEntryKind::Fragment => "fragment",
        GpuEntryKind::Tile => "tile",
        GpuEntryKind::Raygen => "raygen",
        GpuEntryKind::Intersection => "intersection",
        GpuEntryKind::Anyhit => "anyhit",
        GpuEntryKind::Closesthit => "closesthit",
        GpuEntryKind::Miss => "miss",
        GpuEntryKind::Callable => "callable",
        GpuEntryKind::Node => "node",
        GpuEntryKind::Visible => "visible",
    }
}
