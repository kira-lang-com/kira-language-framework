use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind};

mod capabilities;
mod fields;
mod image_metadata;

pub(super) use image_metadata::verify_image_metadata;

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
    for item in image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Entry)
    {
        let Some(entry) = image.gpu_entry_for_item(*item) else {
            continue;
        };
        let (Some(stage), Some(root)) = (entry.kind.as_deref(), entry.function.as_deref()) else {
            diagnostics.push(
                Diagnostic::error(VFY22, "GPU entry record has an invalid header")
                    .with_primary_span(item.span),
            );
            continue;
        };
        if !entry.header_valid {
            diagnostics.push(
                Diagnostic::error(VFY22, "GPU entry record has an invalid header")
                    .with_primary_span(item.span),
            );
            continue;
        }
        if !is_gpu_entry_kind(stage) {
            diagnostics.push(
                Diagnostic::error(VFY22, format!("unknown GPU entry kind '{stage}'"))
                    .with_primary_span(item.span),
            );
            continue;
        }
        diagnostics.extend(fields::verify(entry, stage, 0));
        if !referenced.insert(root.to_owned()) {
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
            verify_entry_signature(stage, entry, signature, item.span, &mut diagnostics);
        }
        if stage != "visible" {
            let mut required = fields::record_capability_requirements(entry, stage);
            let usage = capabilities::analyze(image, root, stage);
            required.extend(usage.capabilities);
            if let Some(declared) = fields::declared_capabilities(entry) {
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
            if let Some(recorded) = fields::specialization_constants(image.source(), entry) {
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
    stage: &str,
    entry: &tvisa_format::TextGpuEntry,
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

    let input_fields = fields::stage_field_types(entry, "stage_input");
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

    let output_fields = fields::stage_field_types(entry, "stage_output");
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

fn is_gpu_entry_kind(stage: &str) -> bool {
    matches!(
        stage,
        "compute"
            | "object"
            | "mesh"
            | "vertex"
            | "fragment"
            | "tile"
            | "raygen"
            | "intersection"
            | "anyhit"
            | "closesthit"
            | "miss"
            | "callable"
            | "node"
            | "visible"
    )
}
