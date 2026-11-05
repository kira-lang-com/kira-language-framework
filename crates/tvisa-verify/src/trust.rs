use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextInstruction, parse_tvisa_function_body};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind};

const VFY50: Code = Code::new(Dialect::Tvisa, Area::Verifier, 50);

#[derive(Clone, Copy, Eq, PartialEq)]
enum TrustClass {
    Managed,
    Sandboxed,
    Unmanaged,
}

impl TrustClass {
    const fn name(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Sandboxed => "sandboxed",
            Self::Unmanaged => "unmanaged",
        }
    }
}

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let foreign_imports = foreign_imports(image);
    let mut diagnostics = Vec::new();

    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Some((declared, span)) = trust_class(source) else {
            diagnostics.push(
                Diagnostic::error(VFY50, "function record requires a valid trust class")
                    .with_primary_span(shift(Span::new(0, source.len()), item.span.start)),
            );
            continue;
        };
        let Ok(Some(body)) = parse_tvisa_function_body(source) else {
            continue;
        };
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            let Some((signature_name, tag)) = indirect_call_signature_tag(instruction) else {
                continue;
            };
            let convention = image
                .signatures
                .iter()
                .find(|signature| signature.name == signature_name)
                .map(|signature| signature.convention);
            if tag == 0 && convention != Some(TextCallingConvention::C) {
                diagnostics.push(
                    Diagnostic::error(
                        VFY50,
                        "an indirect call without a CFI tag must use a C signature",
                    )
                    .with_primary_span(shift(instruction.span, item.span.start)),
                );
            } else if tag != 0 && super::symbols::forbidden_tag(tag) {
                diagnostics.push(
                    Diagnostic::error(
                        VFY50,
                        format!("indirect call uses forbidden type tag 0x{tag:08X}"),
                    )
                    .with_primary_span(shift(instruction.span, item.span.start)),
                );
            }
        }
        let gpu = super::gpu::function_signature_name(source)
            .and_then(|name| {
                image
                    .signatures
                    .iter()
                    .find(|signature| signature.name == name)
            })
            .is_some_and(|signature| {
                matches!(
                    signature.convention,
                    TextCallingConvention::Gpu | TextCallingConvention::Entry
                )
            });

        let has_unmanaged_content = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| {
                has_unmanaged_instruction(instruction)
                    || is_foreign_call(instruction, &foreign_imports)
                    || is_foreign_indirect_call(instruction, image)
            });
        if has_unmanaged_content && declared != TrustClass::Unmanaged {
            diagnostics.push(
                Diagnostic::error(
                    VFY50,
                    format!(
                        "function is declared {} but contains unmanaged instruction content",
                        declared.name()
                    ),
                )
                .with_primary_span(shift(span, item.span.start)),
            );
        }

        if gpu && declared == TrustClass::Sandboxed {
            diagnostics.push(
                Diagnostic::error(VFY50, "GPU functions cannot use the sandboxed trust class")
                    .with_primary_span(shift(span, item.span.start)),
            );
        }
    }

    verify_runtime_entries(image, &mut diagnostics);

    diagnostics
}

fn verify_runtime_entries(image: &TextImage, diagnostics: &mut Vec<Diagnostic>) {
    let mut used = HashSet::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let Ok(Some(body)) = parse_tvisa_function_body(image.item_text(*item)) else {
            continue;
        };
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if let Some(id) = super::profile::runtime_entry_id(instruction) {
                used.insert(id);
            }
        }
    }
    let mut used: Vec<_> = used.into_iter().collect();
    used.sort_unstable();

    let mut records = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Metadata {
            continue;
        }
        let source = image.item_text(*item);
        if !is_runtime_metadata(source) {
            continue;
        }
        if let Some(record) = parse_runtime_metadata(source) {
            records.push((record.0, shift(record.1, item.span.start)));
        } else {
            diagnostics.push(
                Diagnostic::error(VFY50, "malformed metadata runtime item")
                    .with_primary_span(item.span),
            );
        }
    }
    if records.len() > 1 {
        diagnostics.push(
            Diagnostic::error(VFY50, "image has more than one metadata runtime item")
                .with_primary_span(records[1].1),
        );
        return;
    }

    let Some((recorded, span)) = records.into_iter().next() else {
        if !used.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    VFY50,
                    "metadata runtime omits runtime entries used by the image",
                )
                .with_primary_span(Span::new(0, image.source().len())),
            );
        }
        return;
    };
    if recorded.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                VFY50,
                "metadata runtime must be omitted when no runtime entries are used",
            )
            .with_primary_span(span),
        );
    }
    if recorded != used {
        diagnostics.push(
            Diagnostic::error(
                VFY50,
                "metadata runtime does not match call.runtime instructions",
            )
            .with_primary_span(span),
        );
    }
}

fn parse_runtime_metadata(source: &str) -> Option<(Vec<u16>, Span)> {
    let tokens = lex(source).ok()?;
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect();
    if tokens.len() < 4 || atom(tokens[0])? != "metadata" || atom(tokens[1])? != "runtime" {
        return None;
    }
    let span = Span::new(tokens.first()?.span.start, tokens.last()?.span.end);
    if !punctuation(tokens[2], '[') || !punctuation(*tokens.last()?, ']') {
        return None;
    }
    let mut ids = Vec::new();
    let mut index = 3;
    let end = tokens.len() - 1;
    while index < end {
        let value = atom(tokens[index])?;
        let id = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .and_then(|hex| u16::from_str_radix(hex, 16).ok())?;
        if ids.last().is_some_and(|previous| *previous >= id) {
            return None;
        }
        ids.push(id);
        index += 1;
        if index < end {
            if !punctuation(tokens[index], ',') {
                return None;
            }
            index += 1;
            if index == end {
                return None;
            }
        }
    }
    Some((ids, span))
}

fn is_runtime_metadata(source: &str) -> bool {
    let Ok(tokens) = lex(source) else {
        return false;
    };
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect();
    tokens.len() >= 2 && atom(tokens[0]) == Some("metadata") && atom(tokens[1]) == Some("runtime")
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn trust_class(source: &str) -> Option<(TrustClass, Span)> {
    let tokens = lex(source).ok()?;
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let declarations: Vec<_> = tokens[..header_end]
        .windows(2)
        .filter(|pair| atom(&pair[0]) == Some("trust"))
        .collect();
    if declarations.len() != 1 {
        return None;
    }
    let declaration = declarations[0];
    match atom(&declaration[1])? {
        "managed" => Some((TrustClass::Managed, declaration[0].span)),
        "sandboxed" => Some((TrustClass::Sandboxed, declaration[0].span)),
        "unmanaged" => Some((TrustClass::Unmanaged, declaration[0].span)),
        _ => None,
    }
}

fn has_unmanaged_instruction(instruction: &TextInstruction) -> bool {
    let opcode = instruction.opcode.as_str();
    matches!(
        opcode,
        "asm"
            | "asm.goto"
            | "call.variadic"
            | "code.addr"
            | "inttoptr"
            | "setjmp"
            | "longjmp"
            | "target.intrinsic"
            | "va.arg"
            | "va.copy"
            | "va.end"
            | "va.start"
    ) || opcode.starts_with("mmio.")
        || opcode.starts_with("kernel.")
        || matches!(
            super::profile::runtime_entry_id(instruction),
            Some(0x0001 | 0x0004 | 0x0005 | 0x0040 | 0x0041)
        )
}

fn foreign_imports(image: &TextImage) -> HashSet<String> {
    image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Import)
        .filter_map(|item| {
            let source = image.item_text(*item);
            let name = super::gpu_restrictions::item_symbol(source)?;
            let signature_name = super::gpu::function_signature_name(source)?;
            image
                .signatures
                .iter()
                .find(|signature| signature.name == signature_name)
                .is_some_and(|signature| signature.convention == TextCallingConvention::C)
                .then_some(name)
        })
        .collect()
}

fn is_foreign_call(instruction: &TextInstruction, foreign_imports: &HashSet<String>) -> bool {
    if !matches!(instruction.opcode.as_str(), "call" | "tail.call" | "invoke") {
        return false;
    }
    super::symbols::instruction_symbol_operand(instruction)
        .is_some_and(|(name, _)| foreign_imports.contains(&name))
}

fn is_foreign_indirect_call(instruction: &TextInstruction, image: &TextImage) -> bool {
    let Some((signature_name, 0)) = indirect_call_signature_tag(instruction) else {
        return false;
    };
    image
        .signatures
        .iter()
        .find(|signature| signature.name == signature_name)
        .is_some_and(|signature| signature.convention == TextCallingConvention::C)
}

fn indirect_call_signature_tag(instruction: &TextInstruction) -> Option<(String, u32)> {
    if !matches!(
        instruction.opcode.as_str(),
        "call.indirect" | "tail.call" | "invoke"
    ) {
        return None;
    }
    let tokens = &instruction.tokens;
    let signature_start = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('$')))?;
    let TokenKind::Atom(signature_name) = &tokens.get(signature_start + 1)?.kind else {
        return None;
    };
    let tag = tokens.iter().skip(signature_start + 2).find_map(|token| {
        let TokenKind::Atom(value) = &token.kind else {
            return None;
        };
        let digits = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))?;
        u32::from_str_radix(digits, 16).ok()
    })?;
    Some((signature_name.clone(), tag))
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
