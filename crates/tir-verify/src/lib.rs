#![forbid(unsafe_code)]

mod atomic_order;
mod binary_analysis;
mod binary_code;
mod binary_constants;
mod binary_header_flags;
mod binary_references;
mod binary_semantics;
mod binary_variants;
mod borrow_scopes;
mod conversion_types;
mod definite_init;
mod dominators;
mod entry;
mod gpu_profile;
mod instruction_types;
mod model;
mod platform;
mod profile;
mod ridl;
mod runtime_engine;
mod semantics;
mod signature_types;
mod ssa;
mod static_access;
mod structured;
#[cfg(test)]
mod tests;
mod text_cfg;
mod text_layout;
mod text_modifiers;
mod text_types;
mod variant_text;
mod variants;
mod vector_text;

use std::collections::{HashMap, HashSet};

pub use dominators::DominatorTree;
pub use model::{
    Block, Edge, Function, Instruction, Terminator, TerminatorKind, ValueDefinition, ValueUse,
};
pub use ssa::{
    VerifyOptions, terminator_is_s7_forbidden, verify_function, verify_text_value_dominance,
};
pub use text_cfg::text_successors;
use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{
    BinaryModule, FunctionBody, TextItemKind, TextModule, parse_function_body,
    parse_function_signature, text::lex,
};
use tir_target::Platform;

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);
const V3: Code = Code::new(Dialect::Tir, Area::Verifier, 3);
const S7: Code = Code::new(Dialect::Tir, Area::ControlFlow, 7);
const ENCODING: Code = Code::new(Dialect::Tir, Area::Encoding, 1);

/// Verifies the operand clauses on a GPU barrier or memory fence instruction.
///
/// This is shared with TVISA verification so both textual dialects enforce the
/// same synchronization clause rules.
pub fn verify_gpu_sync_instruction(instruction: &tir_format::TextInstruction) -> Vec<Diagnostic> {
    gpu_profile::verify_sync_instruction(instruction)
}

/// Validates the binary module metadata and table records implemented here.
pub fn verify_binary_module_records(module: &BinaryModule, memory_limit: u64) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let module_record = collect_record(module.module_record(memory_limit), &mut diagnostics);
    let strings = collect_record(module.string_table(memory_limit), &mut diagnostics);
    let imports = collect_record(module.imports(memory_limit), &mut diagnostics);
    let types = collect_record(module.type_table(memory_limit), &mut diagnostics);
    let constants = types.as_ref().and_then(|types| {
        collect_record(module.constant_table(memory_limit, types), &mut diagnostics)
    });
    if let (Some(types), Some(constants)) = (&types, &constants) {
        diagnostics.extend(binary_constants::verify_constants(types, constants));
    }
    let globals = types.as_ref().and_then(|types| {
        constants.as_ref().and_then(|constants| {
            collect_record(
                module.global_table(memory_limit, types, constants),
                &mut diagnostics,
            )
        })
    });
    let signatures = types.as_ref().and_then(|types| {
        collect_record(
            module.signature_table(memory_limit, types),
            &mut diagnostics,
        )
    });
    if let (Some(types), Some(signatures)) = (&types, &signatures) {
        diagnostics.extend(ridl::verify_binary_exports(
            signatures,
            types,
            imports.as_deref(),
        ));
    }
    let generics = if let (Some(types), Some(signatures)) = (&types, &signatures) {
        collect_record(
            module.generics_table(memory_limit, types, signatures),
            &mut diagnostics,
        )
    } else {
        None
    };
    let code = signatures.as_ref().and_then(|signatures| {
        collect_record(
            module.code_table(memory_limit, signatures),
            &mut diagnostics,
        )
    });
    let gpu = if let (Some(signatures), Some(types), Some(constants)) =
        (&signatures, &types, &constants)
    {
        collect_record(
            module.gpu_table(memory_limit, signatures, types, constants),
            &mut diagnostics,
        )
    } else {
        None
    };
    let analysis = collect_record(module.analysis_summary(memory_limit), &mut diagnostics);
    if let Some(module_record) = &module_record {
        diagnostics.extend(profile::verify_binary_module_profile(
            module.header.profile,
            module.header.platform,
            module_record.producer.as_ref(),
            module_record.source_hash.is_some(),
        ));
    }
    if let (Some(module_record), Some(signatures), Some(analysis)) =
        (&module_record, &signatures, &analysis)
    {
        diagnostics.extend(binary_header_flags::verify(
            module.header.flags,
            module_record.semantics,
            signatures,
            analysis,
        ));
    }
    if let Some(imports) = &imports {
        diagnostics.extend(platform::verify_binary_platform(
            module.header.profile,
            module.header.platform,
            imports,
            types.as_ref(),
            code.as_ref(),
        ));
    }
    if let (Some(module_record), Some(imports), Some(types), Some(signatures)) =
        (&module_record, &imports, &types, &signatures)
    {
        diagnostics.extend(binary_references::verify_binary_references(
            module_record,
            imports,
            types,
            globals.as_ref(),
            module.header.profile,
            signatures,
        ));
    }
    if let (Some(signatures), Some(code), Some(analysis)) = (&signatures, &code, &analysis) {
        diagnostics.extend(binary_analysis::verify_assembly_inventory(
            code,
            &signatures.records,
            analysis,
        ));
    }
    if let (Some(types), Some(globals), Some(analysis)) = (&types, &globals, &analysis) {
        diagnostics.extend(binary_analysis::verify_sandbox_globals(
            analysis, globals, types,
        ));
    }
    if let (Some(module_record), Some(types), Some(signatures), Some(code), Some(analysis)) =
        (&module_record, &types, &signatures, &code, &analysis)
    {
        diagnostics.extend(binary_analysis::verify_indirect_call_typeids(
            code,
            signatures,
            types,
            analysis,
            &module_record.name,
            generics.as_ref(),
            strings.as_ref(),
            imports.as_deref(),
            globals.as_ref(),
        ));
    }
    if let (Some(module_record), Some(types), Some(signatures), Some(code), Some(analysis)) =
        (&module_record, &types, &signatures, &code, &analysis)
    {
        diagnostics.extend(binary_analysis::verify_known_reason_bits(
            code,
            signatures,
            types,
            analysis,
            module_record.producer.as_ref(),
        ));
    }
    if let (Some(types), Some(signatures), Some(code), Some(analysis)) =
        (&types, &signatures, &code, &analysis)
    {
        if let Some(sets) = collect_record(
            module.variant_sets(memory_limit, signatures),
            &mut diagnostics,
        ) {
            diagnostics.extend(binary_variants::verify(
                &sets,
                &signatures.records,
                types,
                code,
                analysis,
                module.header.platform,
            ));
        }
    }
    if let (Some(types), Some(signatures), Some(code)) = (&types, &signatures, &code) {
        diagnostics.extend(binary_code::verify(
            code,
            signatures,
            types,
            generics.as_ref(),
            strings.as_ref(),
            imports.as_deref(),
            globals.as_ref(),
            gpu.as_ref(),
            module.header.profile,
        ));
    }
    if let (Some(module_record), Some(signatures), Some(code)) =
        (&module_record, &signatures, &code)
    {
        diagnostics.extend(binary_semantics::verify(
            module_record.semantics,
            signatures,
            code,
        ));
    }
    diagnostics
}

fn collect_record<T>(
    result: Result<T, tir_format::FormatError>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<T> {
    match result {
        Ok(record) => Some(record),
        Err(error) => {
            diagnostics.push(error.diagnostic);
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControlFlowOptions {
    pub structured: bool,
    pub gpu: bool,
}

/// Checks block labels, SSA definition uniqueness, terminators and S7.
pub fn verify_control_flow(body: &FunctionBody, options: ControlFlowOptions) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        diagnostics.push(Diagnostic::error(V3, "function body has no basic blocks"));
        return diagnostics;
    }

    let mut labels = HashSet::with_capacity(body.blocks.len());
    let mut definitions = HashMap::new();
    for block in &body.blocks {
        if !labels.insert(block.label.as_str()) {
            diagnostics.push(
                Diagnostic::error(V1, format!("duplicate block label {}", block.label))
                    .with_primary_span(block.span),
            );
        }
        for parameter in &block.parameters {
            record_definition(
                &mut definitions,
                &mut diagnostics,
                &parameter.name,
                parameter.span,
            );
        }
        for instruction in &block.instructions {
            if (options.gpu || options.structured)
                && is_forbidden_in_structured_control_flow(&instruction.opcode)
            {
                diagnostics.push(
                    Diagnostic::error(
                        S7,
                        format!(
                            "{} is forbidden in structured control flow",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
            for result in &instruction.results {
                record_definition(&mut definitions, &mut diagnostics, result, instruction.span);
            }
        }
    }

    for block in &body.blocks {
        if block.instructions.is_empty() {
            diagnostics.push(
                Diagnostic::error(V3, format!("block {} has no terminator", block.label))
                    .with_primary_span(block.span),
            );
            continue;
        }
        let terminators: Vec<_> = block
            .instructions
            .iter()
            .enumerate()
            .filter(|(_, instruction)| is_terminator(&instruction.opcode))
            .collect();
        match terminators.as_slice() {
            [] => diagnostics.push(
                Diagnostic::error(V3, format!("block {} has no terminator", block.label))
                    .with_primary_span(block.span),
            ),
            [(index, terminator)] if *index + 1 == block.instructions.len() => {
                if (options.gpu || options.structured)
                    && is_forbidden_in_structured_control_flow(&terminator.opcode)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            S7,
                            format!(
                                "{} is forbidden in structured control flow",
                                terminator.opcode
                            ),
                        )
                        .with_primary_span(terminator.span),
                    );
                }
            }
            [(index, terminator)] => diagnostics.push(
                Diagnostic::error(
                    V3,
                    format!(
                        "block {} has an instruction after its {} terminator",
                        block.label, terminator.opcode
                    ),
                )
                .with_primary_span(block.instructions[*index + 1].span),
            ),
            [(_, first), ..] => diagnostics.push(
                Diagnostic::error(
                    V3,
                    format!("block {} contains more than one terminator", block.label),
                )
                .with_primary_span(first.span),
            ),
        }
    }
    diagnostics
}

pub fn verify_text_module(module: &TextModule, defaults: ControlFlowOptions) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(profile::verify_text_module_profile(
        module.header.profile,
        module.header.platform,
        module.header.producer.as_ref(),
        module.header.source_hash.as_deref(),
    ));
    diagnostics.extend(platform::verify_text_module(module));
    let (mut layouts, layout_diagnostics) = text_layout::from_module(module);
    diagnostics.extend(layout_diagnostics);
    let policies = gpu_profile::policy::TypePolicies::from_module(module);
    diagnostics.extend(ridl::verify_text_exports(module, &layouts, &policies));
    let call_conventions = signature_types::callable_conventions(module);
    diagnostics.extend(signature_types::verify_module_calls(
        module,
        &layouts,
        &policies,
        &call_conventions,
    ));
    diagnostics.extend(variants::verify_text_module(module));
    diagnostics.extend(entry::verify(module, &mut layouts, &policies));
    diagnostics.extend(gpu_profile::verify_module(module, &mut layouts));
    for item in &module.items {
        if !matches!(item.kind, TextItemKind::Function | TextItemKind::Entry) {
            continue;
        }
        let source = module.item_text(*item);
        let restricted_engine = runtime_engine::is_restricted(source);
        diagnostics.extend(
            profile::verify_header(module.header.profile, item.kind, source)
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
        let signature = if matches!(item.kind, TextItemKind::Function | TextItemKind::Entry) {
            match parse_function_signature(source) {
                Ok(signature) => signature,
                Err(error) => {
                    diagnostics.push(
                        Diagnostic::error(ENCODING, error.message).with_primary_span(Span::new(
                            item.span.start + error.span.start,
                            item.span.start + error.span.end,
                        )),
                    );
                    None
                }
            }
        } else {
            None
        };
        let body = match parse_function_body(source) {
            Ok(Some(body)) => body,
            Ok(None) => {
                diagnostics.extend(
                    semantics::verify_function(module.header.semantics, source, item.kind, None)
                        .into_iter()
                        .map(|mut diagnostic| {
                            shift_diagnostic(&mut diagnostic, item.span.start);
                            diagnostic
                        }),
                );
                continue;
            }
            Err(error) => {
                diagnostics.push(
                    Diagnostic::error(ENCODING, error.message).with_primary_span(Span::new(
                        item.span.start + error.span.start,
                        item.span.start + error.span.end,
                    )),
                );
                continue;
            }
        };
        diagnostics.extend(
            semantics::verify_function(module.header.semantics, source, item.kind, Some(&body))
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
        diagnostics.extend(
            runtime_engine::verify_instructions(&body, restricted_engine)
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
        diagnostics.extend(
            profile::verify_body(
                module.header.profile,
                module.header.platform.unwrap_or(Platform::Portable),
                source,
                &body,
                signature.as_ref(),
            )
            .into_iter()
            .map(|mut diagnostic| {
                shift_diagnostic(&mut diagnostic, item.span.start);
                diagnostic
            }),
        );
        if let Some(signature) = &signature {
            diagnostics.extend(
                signature_types::verify_function(signature, &body)
                    .into_iter()
                    .map(|mut diagnostic| {
                        shift_diagnostic(&mut diagnostic, item.span.start);
                        diagnostic
                    }),
            );
        }
        diagnostics.extend(
            definite_init::verify(&body)
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
        let inferred_types = text_types::inferred_value_types_with_calls(
            source,
            &body,
            &layouts,
            &policies,
            &call_conventions,
        );
        diagnostics.extend(
            text_cfg::verify_edge_types(&body, &inferred_types)
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
        let options = item_options(source, item.kind, defaults);
        diagnostics.extend(
            text_types::verify_function(
                source,
                &body,
                signature.as_ref(),
                &mut layouts,
                &policies,
                &call_conventions,
                atomic_order::gpu_scope_allowed(source, item.kind, options.gpu),
            )
            .into_iter()
            .map(|mut diagnostic| {
                shift_diagnostic(&mut diagnostic, item.span.start);
                diagnostic
            }),
        );
        diagnostics.extend(
            verify_text_body(&body, options)
                .into_iter()
                .map(|mut diagnostic| {
                    shift_diagnostic(&mut diagnostic, item.span.start);
                    diagnostic
                }),
        );
    }
    diagnostics
}

/// Verifies parsed textual TIR or TVISA blocks for control flow and modifiers.
/// Callers select GPU enforcement from the function's resolved signature.
pub fn verify_text_body(body: &FunctionBody, mut options: ControlFlowOptions) -> Vec<Diagnostic> {
    options.structured |= structured::has_annotations(body);
    let mut diagnostics = verify_control_flow(body, options);
    diagnostics.extend(
        body.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .flat_map(text_modifiers::verify_instruction),
    );
    diagnostics.extend(text_cfg::verify_text_cfg(
        body,
        options.gpu || options.structured,
    ));
    diagnostics.extend(verify_text_value_dominance(body));
    diagnostics.extend(borrow_scopes::verify(body));
    diagnostics.extend(static_access::verify(body));
    diagnostics
}

fn shift_diagnostic(diagnostic: &mut Diagnostic, offset: usize) {
    let shift = |span: &mut Span| {
        span.start = span.start.saturating_add(offset);
        span.end = span.end.saturating_add(offset);
    };
    if let Some(span) = &mut diagnostic.primary_span {
        shift(span);
    }
    for label in &mut diagnostic.labels {
        shift(&mut label.span);
    }
    for note in &mut diagnostic.notes {
        if let Some(span) = &mut note.span {
            shift(span);
        }
    }
    for fix in &mut diagnostic.fixes {
        for edit in &mut fix.edits {
            shift(&mut edit.span);
        }
    }
}

fn item_options(
    source: &str,
    kind: TextItemKind,
    defaults: ControlFlowOptions,
) -> ControlFlowOptions {
    let Ok(tokens) = lex(source) else {
        return defaults;
    };
    let header = tokens
        .iter()
        .take_while(|token| !matches!(token.kind, tir_format::text::TokenKind::Punctuation('{')));
    let mut structured = defaults.structured;
    let mut gpu = defaults.gpu || kind == TextItemKind::Entry;
    let mut in_target_attribute = false;
    for token in header {
        match &token.kind {
            tir_format::text::TokenKind::Atom(atom) if atom == "structured" => {
                structured = true;
            }
            tir_format::text::TokenKind::Atom(atom) if atom == "target" => {
                in_target_attribute = true;
            }
            tir_format::text::TokenKind::Punctuation(')') => in_target_attribute = false,
            tir_format::text::TokenKind::Atom(atom) if in_target_attribute && atom == "gpu" => {
                gpu = true;
            }
            _ => {}
        }
    }
    ControlFlowOptions { structured, gpu }
}

fn record_definition(
    definitions: &mut HashMap<String, tir_diag::Span>,
    diagnostics: &mut Vec<Diagnostic>,
    name: &str,
    span: tir_diag::Span,
) {
    if let Some(previous) = definitions.get(name) {
        diagnostics.push(
            Diagnostic::error(V1, format!("SSA value %{name} is defined more than once"))
                .with_primary_span(span)
                .with_note_at(*previous, "the first definition is here"),
        );
    } else {
        definitions.insert(name.to_owned(), span);
    }
}

#[must_use]
pub fn is_terminator(opcode: &str) -> bool {
    matches!(
        opcode,
        "br" | "cond.br"
            | "switch.int"
            | "switch.enum"
            | "br.indirect"
            | "return"
            | "fail"
            | "tail.call"
            | "unreachable"
            | "trap"
            | "try.call"
            | "invoke"
            | "coro.yield"
            | "coro.resume"
            | "coro.finish"
            | "resume"
            | "throw"
            | "longjmp"
            | "asm.goto"
            | "deopt"
            | "rt.ignore_hit"
            | "rt.accept_and_end"
    )
}

fn is_forbidden_in_structured_control_flow(opcode: &str) -> bool {
    matches!(
        opcode,
        "br.indirect" | "invoke" | "landingpad" | "resume" | "throw" | "setjmp" | "longjmp"
    )
}
