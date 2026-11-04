use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind, lex};
use tir_target::Profile;
use tvisa_format::{
    ImportModuleKind, TextCallingConvention, TextImage, TextImportPayload, TextItem, TextItemKind,
};

#[path = "symbols/parsing.rs"]
mod parsing;

use parsing::{parse_function, parse_function_import};

const VFY10: Code = Code::new(Dialect::Tvisa, Area::Verifier, 10);
const VFY11: Code = Code::new(Dialect::Tvisa, Area::Verifier, 11);
const ENCODING: Code = Code::new(Dialect::Tvisa, Area::Encoding, 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SymbolKind {
    Function,
    ImportedFunction,
    PlatformImportedFunction,
    Data,
    ImportedData,
    PlatformImportedData,
    PerCpuData,
    VariantSet,
    DispatchSet,
    GpuEntry,
}

#[derive(Clone, Debug)]
struct FunctionRecord {
    address_taken: bool,
    imported: bool,
    c_abi: bool,
}

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut functions = HashMap::new();
    let mut symbols: HashMap<String, Vec<SymbolKind>> = HashMap::new();
    for item in &image.items {
        match item.kind {
            TextItemKind::Function => {
                if let Some((name, record, mut errors)) = parse_function(image, *item) {
                    shift_all(&mut errors, item.span.start);
                    diagnostics.extend(errors);
                    add_symbol(
                        &mut symbols,
                        &mut diagnostics,
                        name.clone(),
                        SymbolKind::Function,
                        *item,
                    );
                    functions.insert(name, record);
                }
            }
            TextItemKind::Import => {
                if let Some(import) = image.import_for_item(*item) {
                    match &import.payload {
                        TextImportPayload::Function { .. } => {
                            if let Some((name, record, errors)) =
                                parse_function_import(image, import)
                            {
                                diagnostics.extend(errors);
                                let kind = if import.module.kind == ImportModuleKind::Platform {
                                    SymbolKind::PlatformImportedFunction
                                } else {
                                    SymbolKind::ImportedFunction
                                };
                                add_symbol(
                                    &mut symbols,
                                    &mut diagnostics,
                                    name.clone(),
                                    kind,
                                    *item,
                                );
                                functions.insert(name, record);
                            }
                        }
                        TextImportPayload::Data { .. } => {
                            let kind = if import.module.kind == ImportModuleKind::Platform {
                                SymbolKind::PlatformImportedData
                            } else {
                                SymbolKind::ImportedData
                            };
                            add_symbol(
                                &mut symbols,
                                &mut diagnostics,
                                import.symbol.clone(),
                                kind,
                                *item,
                            );
                        }
                    }
                }
            }
            TextItemKind::Data => {
                if let Some(data) = image.data_for_item(*item) {
                    add_symbol(
                        &mut symbols,
                        &mut diagnostics,
                        data.symbol.clone(),
                        if data.per_cpu {
                            SymbolKind::PerCpuData
                        } else {
                            SymbolKind::Data
                        },
                        *item,
                    );
                }
            }
            TextItemKind::Variants => {
                if let Some(name) = named_symbol(image, *item) {
                    add_symbol(
                        &mut symbols,
                        &mut diagnostics,
                        name,
                        SymbolKind::VariantSet,
                        *item,
                    );
                }
            }
            TextItemKind::Dispatch => {
                if let Some(name) = named_symbol(image, *item) {
                    add_symbol(
                        &mut symbols,
                        &mut diagnostics,
                        name,
                        SymbolKind::DispatchSet,
                        *item,
                    );
                }
            }
            TextItemKind::Entry => {
                if let Some(name) = named_symbol(image, *item) {
                    add_symbol(
                        &mut symbols,
                        &mut diagnostics,
                        name,
                        SymbolKind::GpuEntry,
                        *item,
                    );
                }
            }
            TextItemKind::Signature | TextItemKind::Metadata => {}
        }
    }

    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        let mut cursor = 0;
        while cursor < tokens.len() {
            if !is_punctuation(&tokens[cursor], '@') {
                cursor += 1;
                continue;
            }
            let Some((name, next, span)) = read_symbol(&tokens, cursor) else {
                cursor += 1;
                continue;
            };
            if is_func_ref_operand(&tokens, tokens[cursor].span.start)
                && let Some(record) = functions.get(&name)
                && !record.address_taken
            {
                diagnostics.push(
                    Diagnostic::error(VFY11, format!("func.ref @{name} requires address_taken"))
                        .with_primary_span(shift(span, item.span.start)),
                );
            }
            cursor = next;
        }
        if let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        {
            let signature_name = crate::gpu::function_signature_name(source);
            let caller_is_gpu = image
                .signatures
                .iter()
                .find(|signature| Some(signature.name.as_str()) == signature_name.as_deref())
                .is_some_and(|signature| {
                    matches!(
                        signature.convention,
                        TextCallingConvention::Gpu | TextCallingConvention::Entry
                    )
                });
            for block in &body.blocks {
                for instruction in &block.instructions {
                    verify_symbol_operand(
                        &mut diagnostics,
                        &symbols,
                        &functions,
                        image.header.profile,
                        caller_is_gpu,
                        *item,
                        instruction,
                    );
                }
            }
        }
    }
    diagnostics
}

fn verify_symbol_operand(
    diagnostics: &mut Vec<Diagnostic>,
    symbols: &HashMap<String, Vec<SymbolKind>>,
    functions: &HashMap<String, FunctionRecord>,
    profile: Profile,
    caller_is_gpu: bool,
    item: TextItem,
    instruction: &TextInstruction,
) {
    let expected = match instruction.opcode.as_str() {
        "func.ref" => &[
            SymbolKind::Function,
            SymbolKind::ImportedFunction,
            SymbolKind::PlatformImportedFunction,
        ][..],
        "global.addr" => &[
            SymbolKind::Data,
            SymbolKind::ImportedData,
            SymbolKind::PlatformImportedData,
            SymbolKind::PerCpuData,
        ][..],
        "tls.addr" => &[SymbolKind::Data][..],
        "foreign.addr" => &[SymbolKind::PlatformImportedFunction][..],
        "foreign.data" => &[SymbolKind::PlatformImportedData][..],
        "foreign.entry" => &[SymbolKind::Function][..],
        "kernel.percpu.addr" => &[SymbolKind::PerCpuData][..],
        "gpu.entry.ref" => &[SymbolKind::GpuEntry][..],
        "call" | "tail.call" | "invoke" | "call.variadic" => &[
            SymbolKind::Function,
            SymbolKind::ImportedFunction,
            SymbolKind::PlatformImportedFunction,
            SymbolKind::VariantSet,
            SymbolKind::DispatchSet,
        ][..],
        _ => return,
    };
    let Some((name, span)) = instruction_symbol_operand(instruction) else {
        return;
    };
    let Some(kinds) = symbols.get(&name) else {
        diagnostics.push(
            Diagnostic::error(
                VFY10,
                format!("{} names unknown symbol @{name}", instruction.opcode),
            )
            .with_primary_span(shift(span, item.span.start)),
        );
        return;
    };
    if !kinds.iter().any(|kind| expected.contains(kind)) {
        let expected_name = if matches!(
            instruction.opcode.as_str(),
            "global.addr" | "tls.addr" | "foreign.data" | "kernel.percpu.addr"
        ) {
            "data"
        } else if instruction.opcode == "gpu.entry.ref" {
            "GPU entry"
        } else if matches!(
            instruction.opcode.as_str(),
            "func.ref" | "foreign.addr" | "foreign.entry"
        ) {
            "function"
        } else {
            "function, variant set, or dispatch set"
        };
        diagnostics.push(
            Diagnostic::error(
                VFY10,
                format!(
                    "{} operand @{name} must name a {expected_name}",
                    instruction.opcode
                ),
            )
            .with_primary_span(shift(span, item.span.start)),
        );
    }
    if matches!(
        instruction.opcode.as_str(),
        "foreign.addr" | "foreign.entry"
    ) && !functions.get(&name).is_some_and(|function| {
        function.c_abi && function.imported == (instruction.opcode == "foreign.addr")
    }) {
        diagnostics.push(
            Diagnostic::error(
                VFY10,
                format!("{} requires a function with the C ABI", instruction.opcode),
            )
            .with_primary_span(shift(span, item.span.start)),
        );
    }
    if instruction.opcode == "kernel.percpu.addr" && profile != Profile::Kernel {
        diagnostics.push(
            Diagnostic::error(
                VFY10,
                "kernel.percpu.addr is only valid in the kernel profile",
            )
            .with_primary_span(shift(span, item.span.start)),
        );
    }
    if matches!(
        instruction.opcode.as_str(),
        "foreign.addr" | "foreign.data" | "foreign.entry" | "kernel.percpu.addr"
    ) && caller_is_gpu
    {
        diagnostics.push(
            Diagnostic::error(
                VFY10,
                format!("{} is unavailable in GPU code", instruction.opcode),
            )
            .with_primary_span(shift(span, item.span.start)),
        );
    }
}

pub(super) fn instruction_symbol_operand(instruction: &TextInstruction) -> Option<(String, Span)> {
    let start = instruction_operands_start(&instruction.tokens);
    read_symbol(&instruction.tokens, start).map(|(name, _, span)| (name, span))
}

pub(super) fn function_is_exported(source: &str) -> bool {
    let Ok(tokens) = lex(source) else {
        return false;
    };
    let tokens = significant(&tokens);
    tokens
        .get(1)
        .is_some_and(|token| atom(token) == Some("exported"))
}

fn named_symbol(image: &TextImage, item: TextItem) -> Option<String> {
    let all_tokens = lex(image.item_text(item)).ok()?;
    let tokens = significant(&all_tokens);
    let at = tokens.iter().position(|token| is_punctuation(token, '@'))?;
    read_symbol_refs(&tokens, at).map(|(name, _, _)| name)
}

pub(super) fn symbol_references(tokens: &[&Token]) -> Vec<String> {
    let mut references = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        if is_punctuation(tokens[cursor], '@')
            && let Some((name, next, _)) = read_symbol_refs(tokens, cursor)
        {
            references.push(name);
            cursor = next;
        } else {
            cursor += 1;
        }
    }
    references
}

fn add_symbol(
    symbols: &mut HashMap<String, Vec<SymbolKind>>,
    diagnostics: &mut Vec<Diagnostic>,
    name: String,
    kind: SymbolKind,
    item: TextItem,
) {
    let kinds = symbols.entry(name.clone()).or_default();
    if !kinds.is_empty() {
        diagnostics.push(
            Diagnostic::error(VFY10, format!("duplicate TVISA symbol @{name}"))
                .with_primary_span(item.span),
        );
    }
    if !kinds.contains(&kind) {
        kinds.push(kind);
    }
}

fn instruction_operands_start(tokens: &[Token]) -> usize {
    let mut cursor = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        loop {
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            ) || !matches!(
                tokens.get(cursor + 1).map(|token| &token.kind),
                Some(TokenKind::Atom(_))
            ) {
                return 0;
            }
            cursor += 2;
            match tokens.get(cursor).map(|token| &token.kind) {
                Some(TokenKind::Punctuation('=')) => {
                    cursor += 1;
                    break;
                }
                Some(TokenKind::Punctuation(',')) => cursor += 1,
                _ => return 0,
            }
        }
    }
    cursor += 1;
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) && matches!(
        tokens.get(cursor + 1).map(|token| &token.kind),
        Some(TokenKind::Atom(_))
    ) {
        cursor += 2;
    }
    cursor
}

fn parse_tag(tokens: &[&Token], diagnostics: &mut Vec<Diagnostic>) -> Option<u32> {
    let Some(index) = tokens.iter().position(|token| is_atom(token, "tag")) else {
        return None;
    };
    let Some(value_token) = tokens.get(index + 1) else {
        diagnostics.push(
            Diagnostic::error(VFY11, "tag is missing its 32-bit value")
                .with_primary_span(tokens[index].span),
        );
        return None;
    };
    let Some(value) = atom(value_token).and_then(parse_hex_u32) else {
        diagnostics.push(
            Diagnostic::error(VFY11, "tag must be a 32-bit hexadecimal value")
                .with_primary_span(value_token.span),
        );
        return None;
    };
    Some(value)
}

fn has_attribute(tokens: &[&Token], expected: &str) -> bool {
    let Some(index) = tokens.iter().position(|token| is_atom(token, "attrs")) else {
        return false;
    };
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(index + 1)
        .find_map(|(index, token)| is_punctuation(token, '(').then_some(index))
    else {
        return false;
    };
    let Some(close) = tokens
        .iter()
        .enumerate()
        .skip(open + 1)
        .find_map(|(index, token)| is_punctuation(token, ')').then_some(index))
    else {
        return false;
    };
    tokens[open + 1..close]
        .iter()
        .any(|token| is_atom(token, expected))
}

fn is_func_ref_operand(tokens: &[Token], at_offset: usize) -> bool {
    let Some(at_index) = tokens
        .iter()
        .position(|token| token.span.start == at_offset)
    else {
        return false;
    };
    at_index >= 3
        && is_atom(&tokens[at_index - 3], "func")
        && is_punctuation(&tokens[at_index - 2], '.')
        && is_atom(&tokens[at_index - 1], "ref")
}

fn forbidden_tag(tag: u32) -> bool {
    (tag & 0x000F_FFFF) == 0
        || (tag & 0xFFF) == 0x017
        || tag == 0xFA1E_0FF3
        || tag.wrapping_neg() == 0xFA1E_0FF3
        || matches!(
            tag,
            0xD503_241F | 0xD503_245F | 0xD503_249F | 0xD503_24DF | 0xD503_233F | 0xD503_237F
        )
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(
        value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))?,
        16,
    )
    .ok()
}

fn significant(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect()
}

fn shift_all(diagnostics: &mut [Diagnostic], offset: usize) {
    for diagnostic in diagnostics {
        if let Some(span) = &mut diagnostic.primary_span {
            *span = shift(*span, offset);
        }
    }
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}

fn read_symbol(tokens: &[Token], index: usize) -> Option<(String, usize, Span)> {
    if !tokens
        .get(index)
        .is_some_and(|token| is_punctuation(token, '@'))
    {
        return None;
    }
    let first = tokens.get(index + 1)?;
    let mut name = atom(first)?.to_owned();
    let start = tokens[index].span.start;
    let mut end = first.span.end;
    let mut cursor = index + 2;
    while tokens
        .get(cursor)
        .is_some_and(|token| is_punctuation(token, '.'))
    {
        let component = atom(tokens.get(cursor + 1)?)?;
        name.push('.');
        name.push_str(component);
        end = tokens[cursor + 1].span.end;
        cursor += 2;
    }
    Some((name, cursor, Span::new(start, end)))
}

pub(super) fn read_symbol_refs(tokens: &[&Token], index: usize) -> Option<(String, usize, Span)> {
    if !tokens
        .get(index)
        .is_some_and(|token| is_punctuation(token, '@'))
    {
        return None;
    }
    let first = *tokens.get(index + 1)?;
    let mut name = atom(first)?.to_owned();
    let start = tokens[index].span.start;
    let mut end = first.span.end;
    let mut cursor = index + 2;
    while tokens
        .get(cursor)
        .is_some_and(|token| is_punctuation(token, '.'))
    {
        let component = atom(tokens.get(cursor + 1)?)?;
        name.push('.');
        name.push_str(component);
        end = tokens[cursor + 1].span.end;
        cursor += 2;
    }
    Some((name, cursor, Span::new(start, end)))
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_atom(token: &Token, expected: &str) -> bool {
    atom(token) == Some(expected)
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
