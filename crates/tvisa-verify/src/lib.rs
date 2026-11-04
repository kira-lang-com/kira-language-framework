#![forbid(unsafe_code)]

mod assembly_constraints;
mod atomic_types;
mod binary_analysis;
mod binary_code;
mod binary_debug;
mod binary_gpu_entries;
mod binary_gpu_stages;
mod binary_gpu_type_capabilities;
mod binary_gpu_uniformity;
mod binary_gpu_uniformity_cfg;
mod binary_metadata;
mod binary_signatures;
mod binary_symbols;
mod call_types;
mod calls;
mod constant_types;
mod conversion_types;
mod data;
mod debug_types;
mod edge_types;
mod gpu;
mod gpu_advanced_types;
mod gpu_entries;
mod gpu_ray_types;
mod gpu_restrictions;
mod gpu_texture_types;
mod gpu_types;
mod gpu_vft_types;
mod instruction_arity;
mod instruction_set;
mod instruction_types;
mod intrinsic_types;
mod math_types;
mod permissive;
mod profile;
mod runtime;
mod sampler;
mod signature;
mod slots;
mod symbols;
mod text_gpu_uniformity;
mod trust;
mod type_classes;
mod type_tags;
mod unwind;
mod variant_text;
mod variants;
mod vector_inference;
mod vector_types;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_verify::{ControlFlowOptions, verify_text_body, verify_text_value_dominance};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind, TvisaImage};

const ENCODING: Code = Code::new(Dialect::Tvisa, Area::Encoding, 1);
const VFY1: Code = Code::new(Dialect::Tvisa, Area::Verifier, 1);

#[derive(Clone, Copy)]
enum Convention {
    Cpu,
    Gpu,
}

/// Verifies textual TVISA function bodies for block structure and CFG rules.
/// Signature and instruction type verification are handled by later verifier passes.
pub fn verify_text_image(image: &TextImage) -> Vec<Diagnostic> {
    let mut diagnostics = signature::verify_image_signatures(image);
    diagnostics.extend(
        gpu::verify_image_signatures(image)
            .into_iter()
            .map(|diagnostic| map_tir_diagnostic(diagnostic, 0)),
    );
    diagnostics.extend(symbols::verify_image(image));
    diagnostics.extend(data::verify_image(image));
    diagnostics.extend(variants::verify_image(image));
    let call_signatures = calls::SignatureIndex::new(image);
    let uniformity_signatures = text_gpu_uniformity::function_signatures(image);
    let mut uniformity_functions = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let signature = match function_signature(source) {
            Ok(Some((name, span))) => match image
                .signatures
                .iter()
                .find(|signature| signature.name == name)
            {
                Some(signature) => signature,
                None => {
                    diagnostics.push(
                        Diagnostic::error(VFY1, format!("unknown TVISA signature ${name}"))
                            .with_primary_span(shift(span, item.span.start)),
                    );
                    continue;
                }
            },
            Ok(None) => {
                diagnostics.push(
                    Diagnostic::error(VFY1, "function record is missing its signature reference")
                        .with_primary_span(item.span),
                );
                continue;
            }
            Err(error) => {
                diagnostics.push(shift_diagnostic(error, item.span.start));
                continue;
            }
        };
        let Some(parsed_body) = image.function_body_for_item(*item) else {
            continue;
        };
        let body = match (&parsed_body.body, &parsed_body.error) {
            (Some(body), _) => body,
            (None, Some(error)) => {
                diagnostics.push(
                    Diagnostic::error(ENCODING, error.message.clone())
                        .with_primary_span(shift(error.span, item.span.start)),
                );
                continue;
            }
            (None, None) => {
                diagnostics.push(
                    Diagnostic::error(VFY1, "defined TVISA function has no body")
                        .with_primary_span(item.span),
                );
                continue;
            }
        };
        let (uniformity_function, uniformity_diagnostics) = text_gpu_uniformity::analyze_function(
            source,
            &body,
            signature,
            item.span.start,
            &uniformity_signatures,
        );
        diagnostics.extend(uniformity_diagnostics);
        if let Some(function) = uniformity_function {
            uniformity_functions.push(function);
        }
        diagnostics.extend(
            instruction_set::verify_function(&body)
                .into_iter()
                .map(|diagnostic| shift_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(verify_entry_parameters(signature, &body));
        diagnostics.extend(
            profile::verify_function(image.header.profile, source, signature, &body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            permissive::verify_function(source, &body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            gpu::verify_function(signature, &body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            slots::verify_function(source, signature, &body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            unwind::verify_function(&body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            call_signatures
                .verify_function(image, signature, source, &body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            runtime::verify_function(&body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        let convention = match signature.convention {
            TextCallingConvention::Gpu | TextCallingConvention::Entry => Convention::Gpu,
            TextCallingConvention::Ridl
            | TextCallingConvention::C
            | TextCallingConvention::Rtcc => Convention::Cpu,
        };
        let options = ControlFlowOptions {
            structured: matches!(convention, Convention::Gpu),
            gpu: matches!(convention, Convention::Gpu),
        };
        diagnostics.extend(
            verify_text_body(&body, options)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
        diagnostics.extend(
            verify_text_value_dominance(&body)
                .into_iter()
                .map(|diagnostic| map_tir_diagnostic(diagnostic, item.span.start)),
        );
    }
    diagnostics.extend(text_gpu_uniformity::verify_calls(
        image,
        &uniformity_functions,
    ));
    diagnostics.extend(trust::verify_image(image));
    diagnostics.extend(gpu_entries::verify_image_metadata(image));
    diagnostics.extend(gpu_restrictions::verify_image(image));
    diagnostics.extend(gpu::verify_call_graph(image));
    diagnostics.extend(gpu_entries::verify_entry_records(image));
    diagnostics.extend(gpu::verify_builtin_stages(image));
    diagnostics
}

/// Verifies binary DATA records and relocation symbol spaces (VFY-0009/0010).
/// The memory limit bounds the uncompressed STRINGS and DATA sections read here.
pub fn verify_binary_data_section(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    data::verify_binary(image, memory_limit)
}

/// Runs the currently implemented binary DATA, symbol, variant, GPU-entry,
/// and CODE decoding checks.
pub fn verify_binary_image(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let mut diagnostics = data::verify_binary(image, memory_limit);
    diagnostics.extend(binary_symbols::verify_binary_records(image, memory_limit));
    diagnostics.extend(binary_code::verify(image, memory_limit));
    diagnostics.extend(binary_debug::verify(image, memory_limit));
    diagnostics.extend(binary_metadata::verify(image, memory_limit));
    diagnostics.extend(binary_gpu_entries::verify(image, memory_limit));
    diagnostics.extend(binary_gpu_stages::verify(image, memory_limit));
    diagnostics.extend(binary_gpu_uniformity::verify(image, memory_limit));
    diagnostics.extend(variants::verify_binary(image, memory_limit));
    diagnostics.extend(runtime::verify_binary_section(image, memory_limit));
    diagnostics
}

/// Verifies binary variant membership, signatures, feature sets and the
/// architecture coverage rule when the image contains no inline assembly.
pub fn verify_binary_variant_section(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    variants::verify_binary(image, memory_limit)
}

/// Checks binary function attributes, header flags, type tags and GPU entries.
pub fn verify_binary_function_records(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    binary_symbols::verify_binary_records(image, memory_limit)
}

fn function_signature(source: &str) -> Result<Option<(String, Span)>, Diagnostic> {
    let tokens = significant_tokens(source).map_err(|error| {
        Diagnostic::error(ENCODING, error.message.to_owned()).with_primary_span(error.span)
    })?;
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    for index in 0..header_end.saturating_sub(2) {
        if matches!(&tokens[index].kind, TokenKind::Atom(atom) if atom == "sig")
            && matches!(tokens[index + 1].kind, TokenKind::Punctuation('$'))
            && let TokenKind::Atom(name) = &tokens[index + 2].kind
        {
            return Ok(Some((name.clone(), tokens[index + 2].span)));
        }
    }
    Ok(None)
}

fn verify_entry_parameters(
    signature: &tvisa_format::TextSignature,
    body: &tir_format::FunctionBody,
) -> Vec<Diagnostic> {
    let Some(entry) = body.blocks.first() else {
        return Vec::new();
    };
    let expected = call_types::signature_parameter_types(signature, signature.parameters.len());
    let mut diagnostics = Vec::new();
    if entry.parameters.len() != expected.len() {
        diagnostics.push(
            Diagnostic::error(
                VFY1,
                format!(
                    "entry block has {} parameters; signature has {} leaves",
                    entry.parameters.len(),
                    expected.len()
                ),
            )
            .with_primary_span(entry.span),
        );
        return diagnostics;
    }
    for (parameter, expected_type) in entry.parameters.iter().zip(expected) {
        if parameter.ty != expected_type {
            diagnostics.push(
                Diagnostic::error(
                    VFY1,
                    format!(
                        "entry parameter %{} has type {}; signature requires {}",
                        parameter.name, parameter.ty, expected_type
                    ),
                )
                .with_primary_span(parameter.span),
            );
        }
    }
    diagnostics
}

fn significant_tokens(source: &str) -> Result<Vec<Token>, tir_format::text::TextError> {
    Ok(lex(source)?
        .into_iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect())
}

fn map_tir_diagnostic(mut diagnostic: Diagnostic, offset: usize) -> Diagnostic {
    diagnostic.code = Code::new(
        Dialect::Tvisa,
        diagnostic.code.area(),
        diagnostic.code.number(),
    );
    shift_diagnostic(diagnostic, offset)
}

fn shift_diagnostic(mut diagnostic: Diagnostic, offset: usize) -> Diagnostic {
    if let Some(span) = &mut diagnostic.primary_span {
        *span = shift(*span, offset);
    }
    for label in &mut diagnostic.labels {
        label.span = shift(label.span, offset);
    }
    for note in &mut diagnostic.notes {
        if let Some(span) = &mut note.span {
            *span = shift(*span, offset);
        }
    }
    for fix in &mut diagnostic.fixes {
        for edit in &mut fix.edits {
            edit.span = shift(edit.span, offset);
        }
    }
    diagnostic
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
