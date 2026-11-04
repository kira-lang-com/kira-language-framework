use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind, lex};
use tvisa_format::{TextCallingConvention, TextImage, TextImportPayload, TextItemKind};

const VFY50: Code = Code::new(Dialect::Tvisa, Area::Verifier, 50);

#[derive(Clone, Copy, Eq, PartialEq)]
enum TrustClass {
    Managed,
    Sandboxed,
    Unmanaged,
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
        let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        else {
            continue;
        };
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
                has_unmanaged_instruction(instruction, gpu)
                    || is_foreign_call(instruction, &foreign_imports, &image.signatures)
            });
        let has_sandbox_incompatible_content = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| {
                is_sandbox_incompatible_instruction(instruction)
                    || is_foreign_call(instruction, &foreign_imports, &image.signatures)
            });
        if has_unmanaged_content && declared == TrustClass::Managed {
            diagnostics.push(
                Diagnostic::error(
                    VFY50,
                    "function is declared managed but contains unmanaged instruction content",
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
        if declared == TrustClass::Sandboxed && has_sandbox_incompatible_content {
            diagnostics.push(
                Diagnostic::error(
                    VFY50,
                    "sandboxed function contains content the Wasm64 converter cannot emit",
                )
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
        let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        else {
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

    if image.runtime_metadata.len() > 1 {
        diagnostics.push(
            Diagnostic::error(VFY50, "image has more than one metadata runtime item")
                .with_primary_span(image.runtime_metadata[1].span),
        );
        return;
    }

    let Some(record) = image.runtime_metadata.first() else {
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
    if record.identifiers.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                VFY50,
                "metadata runtime must be omitted when no runtime entries are used",
            )
            .with_primary_span(record.span),
        );
    }
    if record.identifiers != used {
        diagnostics.push(
            Diagnostic::error(
                VFY50,
                "metadata runtime does not match call.runtime instructions",
            )
            .with_primary_span(record.span),
        );
    }
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
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

fn has_unmanaged_instruction(instruction: &TextInstruction, gpu: bool) -> bool {
    let opcode = instruction.opcode.as_str();
    matches!(
        opcode,
        "asm"
            | "asm.goto"
            | "into.raw"
            | "from.raw"
            | "call.variadic"
            | "heap.alloc.raw"
            | "heap.realloc.raw"
            | "heap.free.raw"
            | "inttoptr"
            | "setjmp"
            | "longjmp"
            | "target.intrinsic"
            | "va.arg"
            | "va.copy"
            | "va.end"
            | "va.start"
    ) || (!gpu
        && matches!(
            opcode,
            "raw.load"
                | "raw.store"
                | "ptr.offset"
                | "ptr.diff"
                | "ptr.toaddr"
                | "addr.toptr"
                | "mem.copy"
                | "mem.move"
                | "mem.set"
        ))
        || (opcode == "access.begin"
            && instruction
                .tokens
                .iter()
                .any(|token| atom(token) == Some("unchecked")))
        || opcode.starts_with("mmio.")
        || opcode.starts_with("kernel.")
        || matches!(
            super::profile::runtime_entry_id(instruction),
            Some(0x0001 | 0x0004 | 0x0005 | 0x0040 | 0x0041)
        )
}

fn is_sandbox_incompatible_instruction(instruction: &TextInstruction) -> bool {
    matches!(
        instruction.opcode.as_str(),
        "asm"
            | "asm.goto"
            | "call.variadic"
            | "foreign.addr"
            | "foreign.data"
            | "foreign.entry"
            | "heap.alloc"
            | "heap.alloc.counted"
            | "heap.alloc.unique"
            | "heap.alloc.raw"
            | "heap.realloc.raw"
            | "heap.free.raw"
            | "inttoptr"
            | "kernel.percpu.addr"
            | "longjmp"
            | "setjmp"
            | "target.intrinsic"
            | "va.arg"
            | "va.copy"
            | "va.end"
            | "va.start"
    ) || (instruction.opcode == "access.begin"
        && instruction
            .tokens
            .iter()
            .any(|token| atom(token) == Some("unchecked")))
        || instruction.opcode.starts_with("mmio.")
        || instruction.opcode.starts_with("kernel.")
        || super::profile::runtime_entry_id(instruction)
            .is_some_and(|id| (1..=0xf).contains(&id) || matches!(id, 0x0040 | 0x0041))
}

fn foreign_imports(image: &TextImage) -> HashSet<String> {
    image
        .imports
        .iter()
        .filter_map(|import| {
            let TextImportPayload::Function { signature, .. } = &import.payload else {
                return None;
            };
            image
                .signatures
                .iter()
                .find(|item| item.name == *signature)
                .is_some_and(|signature| signature.convention == TextCallingConvention::C)
                .then_some(import.symbol.clone())
        })
        .collect()
}

fn is_foreign_call(
    instruction: &TextInstruction,
    foreign_imports: &HashSet<String>,
    signatures: &[tvisa_format::TextSignature],
) -> bool {
    if matches!(
        instruction.opcode.as_str(),
        "call.indirect" | "tail.call" | "invoke"
    ) && crate::type_tags::text_type_tag(instruction, 0) == Some(0)
        && instruction
            .tokens
            .windows(2)
            .find_map(|pair| {
                (matches!(pair[0].kind, TokenKind::Punctuation('$')))
                    .then(|| atom(&pair[1]))
                    .flatten()
            })
            .and_then(|name| signatures.iter().find(|signature| signature.name == name))
            .is_some_and(|signature| signature.convention == TextCallingConvention::C)
    {
        return true;
    }
    matches!(instruction.opcode.as_str(), "call" | "tail.call" | "invoke")
        && super::symbols::instruction_symbol_operand(instruction)
            .is_some_and(|(name, _)| foreign_imports.contains(&name))
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
