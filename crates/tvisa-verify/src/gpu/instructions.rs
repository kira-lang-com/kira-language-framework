use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::TokenKind;

use super::opcode_end;

const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);

pub(super) fn verify_instruction(instruction: &TextInstruction, diagnostics: &mut Vec<Diagnostic>) {
    let opcode = instruction.opcode.as_str();
    diagnostics.extend(
        tir_verify::verify_gpu_sync_instruction(instruction)
            .into_iter()
            .map(|diagnostic| crate::map_tir_diagnostic(diagnostic, 0)),
    );
    let forbidden_tail_call = opcode == "tail.call" && has_indirect_or_runtime_callee(instruction);
    let forbidden = forbidden_tail_call
        || matches!(
            opcode,
            "asm"
                | "asm.goto"
                | "block.addr"
                | "call.foreign"
                | "call.indirect"
                | "call.runtime"
                | "call.variadic"
                | "code.addr"
                | "code.fromaddr"
                | "code.load"
                | "code.store"
                | "frame.address"
                | "func.ref"
                | "gpu.entry.ref"
                | "landingpad"
                | "longjmp"
                | "resume"
                | "return.address"
                | "stack.alloc.dyn"
                | "stack.restore"
                | "stack.save"
                | "target.has"
                | "target.intrinsic"
                | "target.pagesize"
                | "thread.pointer"
                | "tls.addr"
                | "va.arg"
                | "va.copy"
                | "va.end"
                | "va.start"
                | "atomic.wait"
                | "atomic.notify"
                | "invoke"
                | "setjmp"
                | "throw"
                | "vscale.fixed"
        )
        || opcode.starts_with("fp.flags.")
        || (opcode == "intrinsic" && !matches!(intrinsic_id(instruction), Some(15 | 16)));
    if forbidden {
        diagnostics.push(
            Diagnostic::error(
                VFY21,
                format!("instruction {opcode} is forbidden in the GPU profile"),
            )
            .with_primary_span(instruction.span),
        );
    }

    for (index, token) in instruction.tokens.iter().enumerate() {
        let TokenKind::Atom(atom) = &token.kind else {
            continue;
        };
        let preceded_by_name_sigil = matches!(
            instruction
                .tokens
                .get(index.wrapping_sub(1))
                .map(|token| &token.kind),
            Some(TokenKind::Punctuation('%' | '@' | '$' | '.'))
        );
        let is_pointer_opcode = matches!(
            instruction.tokens.get(index + 1).map(|token| &token.kind),
            Some(TokenKind::Punctuation('.'))
        );
        if atom == "ptr"
            && !preceded_by_name_sigil
            && !is_pointer_opcode
            && !is_gpu_pointer_type(&instruction.tokens, index)
        {
            diagnostics.push(
                Diagnostic::error(VFY21, "GPU code must use an explicit pointer address space")
                    .with_primary_span(token.span),
            );
        }
        if is_type_name(atom)
            && !matches!(
                instruction
                    .tokens
                    .get(index.wrapping_sub(1))
                    .map(|token| &token.kind),
                Some(TokenKind::Punctuation('%' | '@' | '$' | '.'))
            )
        {
            if let Some(message) = forbidden_type_atom(atom) {
                diagnostics.push(Diagnostic::error(VFY21, message).with_primary_span(token.span));
            }
        }
        if atom == "vec"
            && matches!(
                instruction.tokens.get(index + 1).map(|token| &token.kind),
                Some(TokenKind::Punctuation('<'))
            )
            && instruction
                .tokens
                .get(index + 2)
                .and_then(|token| match &token.kind {
                    TokenKind::Atom(lanes) => {
                        tir_format::numeric_literals::parse_unsigned_integer(lanes)
                            .and_then(|lanes| u32::try_from(lanes).ok())
                    }
                    _ => None,
                })
                .is_some_and(|lanes| !matches!(lanes, 2 | 3 | 4))
        {
            diagnostics.push(
                Diagnostic::error(VFY21, "GPU fixed vectors must have 2, 3, or 4 lanes")
                    .with_primary_span(token.span),
            );
        }
    }
}

fn has_indirect_or_runtime_callee(instruction: &TextInstruction) -> bool {
    let operands = &instruction.tokens[super::opcode_end(instruction)..];
    let end = operands
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('(')))
        .unwrap_or(operands.len());
    operands[..end].iter().any(|token| {
        matches!(token.kind, TokenKind::Punctuation('%'))
            || matches!(&token.kind, TokenKind::Atom(name) if name == "rt")
    })
}

fn is_gpu_pointer_type(tokens: &[tir_format::text::Token], index: usize) -> bool {
    let Some(TokenKind::Punctuation('<')) = tokens.get(index + 1).map(|token| &token.kind) else {
        return false;
    };
    let Some(TokenKind::Atom(space)) = tokens.get(index + 2).map(|token| &token.kind) else {
        return false;
    };
    matches!(
        space.as_str(),
        "device"
            | "constant"
            | "threadgroup"
            | "thread"
            | "object_payload"
            | "ray_payload"
            | "hit_attribute"
            | "tile"
            | "node_record"
    )
}

fn is_type_name(atom: &str) -> bool {
    atom == "i128" || atom == "f128" || atom == "svec"
}

fn forbidden_type_atom(atom: &str) -> Option<&'static str> {
    match atom {
        "i128" => Some("GPU code does not support integers wider than 64 bits"),
        "f128" => Some("GPU code does not support f128"),
        "svec" => Some("GPU code cannot use scalable vectors"),
        _ => None,
    }
}

fn intrinsic_id(instruction: &TextInstruction) -> Option<u32> {
    let index = opcode_end(instruction);
    let TokenKind::Atom(value) = &instruction.tokens.get(index)?.kind else {
        return None;
    };
    u32::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
}
