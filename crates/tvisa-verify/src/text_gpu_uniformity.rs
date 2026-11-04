use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::FunctionBody;
use tir_types::Type;
use tir_uniformity::Uniformity;
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind, TextSignature};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub(super) struct FunctionInfo {
    name: String,
    control: Vec<Uniformity>,
    required_on_entry: Option<Uniformity>,
    calls: Vec<CallSite>,
}

struct CallSite {
    block: usize,
    span: Span,
    target: Option<String>,
    vft_type: Option<tir_types::FunctionType>,
}

pub(super) fn function_signatures<'a>(image: &'a TextImage) -> HashMap<String, &'a TextSignature> {
    image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Function)
        .filter_map(|item| {
            let source = image.item_text(*item);
            let name = super::gpu::function_symbol(source)?;
            let signature_name = super::gpu::function_signature_name(source)?;
            let signature = image
                .signatures
                .iter()
                .find(|signature| signature.name == signature_name)?;
            Some((name, signature))
        })
        .collect()
}

pub(super) fn analyze_function(
    source: &str,
    body: &FunctionBody,
    signature: &TextSignature,
    source_offset: usize,
    signatures: &HashMap<String, &TextSignature>,
) -> (Option<FunctionInfo>, Vec<Diagnostic>) {
    if !matches!(
        signature.convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    ) {
        return (None, Vec::new());
    }
    let analysis = tir_uniformity::analyze_gpu_function(
        source,
        body,
        signature.convention == TextCallingConvention::Entry,
    );
    let diagnostics = analysis
        .diagnostics
        .into_iter()
        .map(|diagnostic| super::map_tir_diagnostic(diagnostic, source_offset))
        .collect();
    let Some(name) = super::gpu::function_symbol(source) else {
        return (None, diagnostics);
    };
    let value_types = infer_value_types(source, body, signature, signatures);
    let mut calls = Vec::new();
    for (block, basic_block) in body.blocks.iter().enumerate() {
        for instruction in &basic_block.instructions {
            let vft_type = if instruction.opcode == "gpu.call.vft" {
                value_operands(instruction)
                    .first()
                    .and_then(|table| value_types.get(*table))
                    .and_then(|ty| match ty {
                        Type::VisibleFunctionTable(function) => Some(function.as_ref().clone()),
                        _ => None,
                    })
            } else {
                None
            };
            let target = (instruction.opcode != "gpu.call.vft")
                .then(|| super::gpu::direct_call_target(instruction))
                .flatten();
            if target.is_none() && instruction.opcode != "gpu.call.vft" {
                continue;
            }
            calls.push(CallSite {
                block,
                span: shift_span(instruction.span, source_offset),
                target,
                vft_type,
            });
        }
    }
    (
        Some(FunctionInfo {
            name,
            control: analysis.control,
            required_on_entry: analysis.required_on_entry,
            calls,
        }),
        diagnostics,
    )
}

pub(super) fn verify_calls(image: &TextImage, functions: &[FunctionInfo]) -> Vec<Diagnostic> {
    let function_by_name = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let visible_targets = image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Entry)
        .filter_map(|item| {
            let entry = image.gpu_entry_for_item(*item)?;
            let kind = entry.kind.as_deref()?;
            let target = entry.function.as_ref()?;
            (entry.header_valid && kind == "visible")
                .then(|| (target.clone(), visible_type_id(entry)))
        })
        .filter(|(target, _)| function_by_name.contains_key(target.as_str()))
        .collect::<Vec<_>>();

    let mut summaries = functions
        .iter()
        .map(|function| function.required_on_entry)
        .collect::<Vec<_>>();
    loop {
        let previous = summaries.clone();
        let mut changed = false;
        for (index, function) in functions.iter().enumerate() {
            for call in &function.calls {
                let requirement = target_indexes(call, &function_by_name, &visible_targets)
                    .into_iter()
                    .filter_map(|target| previous.get(target).copied().flatten())
                    .min();
                let Some(requirement) = requirement else {
                    continue;
                };
                let raised =
                    previous[index].map_or(requirement, |current| current.min(requirement));
                if summaries[index] != Some(raised) {
                    summaries[index] = Some(raised);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut diagnostics = Vec::new();
    for function in functions {
        for call in &function.calls {
            let requirement = target_indexes(call, &function_by_name, &visible_targets)
                .into_iter()
                .filter_map(|target| summaries.get(target).copied().flatten())
                .min();
            let Some(required) = requirement else {
                continue;
            };
            if function
                .control
                .get(call.block)
                .is_some_and(|control| control.satisfies(required))
            {
                continue;
            }
            let target = call.target.as_deref().unwrap_or("visible function table");
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!(
                        "GPU call to {target} requires {required:?}-uniform control flow in function @{}",
                        function.name
                    ),
                )
                .with_primary_span(call.span),
            );
        }
    }
    diagnostics
}

fn target_indexes(
    call: &CallSite,
    function_by_name: &HashMap<&str, usize>,
    visible_targets: &[(String, Option<u64>)],
) -> Vec<usize> {
    if let Some(target) = &call.target {
        return function_by_name
            .get(target.as_str())
            .copied()
            .into_iter()
            .collect();
    }
    let type_id = call.vft_type.as_ref().and_then(function_type_id);
    visible_targets
        .iter()
        .filter(|(_, target_type_id)| {
            type_id.is_none_or(|type_id| target_type_id.is_none_or(|target| target == type_id))
        })
        .filter_map(|(target, _)| function_by_name.get(target.as_str()).copied())
        .collect()
}

fn visible_type_id(entry: &tvisa_format::TextGpuEntry) -> Option<u64> {
    let value = entry
        .fields
        .iter()
        .find(|field| field.name == "typeid")
        .and_then(|field| match &field.value.kind {
            tvisa_format::TextGpuEntryValueKind::Scalar(value) => Some(value.as_str()),
            _ => None,
        })?;
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
}

fn function_type_id(function: &tir_types::FunctionType) -> Option<u64> {
    Type::Function(function.clone())
        .type_id()
        .ok()
        .map(|type_id| type_id.as_u64())
}

fn infer_value_types(
    source: &str,
    body: &FunctionBody,
    signature: &TextSignature,
    signatures: &HashMap<String, &TextSignature>,
) -> HashMap<String, Type> {
    let mut value_types = HashMap::new();
    for parameter in body.blocks.iter().flat_map(|block| &block.parameters) {
        value_types.insert(parameter.name.clone(), parameter.ty.clone());
    }
    let slot_types = super::slots::pointer_types(source, signature, body);
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if let Some(target) = super::gpu::direct_call_target(instruction)
            && let Some(signature) = signatures.get(&target)
        {
            value_types.extend(
                instruction
                    .results
                    .iter()
                    .cloned()
                    .zip(super::call_types::signature_result_types(signature)),
            );
        }
        value_types.extend(super::call_types::infer_instruction_results(
            source,
            instruction,
            &value_types,
            &slot_types,
            true,
        ));
    }
    value_types
}

fn value_operands(instruction: &tir_format::TextInstruction) -> Vec<&str> {
    let start = super::calls::opcode_end(instruction);
    instruction.tokens[start..]
        .windows(2)
        .filter_map(|window| {
            matches!(
                window[0].kind,
                tir_format::text::TokenKind::Punctuation('%')
            )
            .then(|| match &window[1].kind {
                tir_format::text::TokenKind::Atom(name) => Some(name.as_str()),
                _ => None,
            })
            .flatten()
        })
        .collect()
}

fn shift_span(mut span: Span, offset: usize) -> Span {
    span.start = span.start.saturating_add(offset);
    span.end = span.end.saturating_add(offset);
    span
}
