use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{BinaryCodeTable, BinaryInstruction, BinarySignatureTable, SemanticsMode};

const V14: Code = Code::new(Dialect::Tir, Area::Verifier, 14);

pub(super) fn verify(
    module_mode: SemanticsMode,
    signatures: &BinarySignatureTable,
    code: &BinaryCodeTable,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for body in &code.functions {
        let Some(signature) = signatures
            .records
            .get(usize::try_from(body.function_index).unwrap_or(usize::MAX))
        else {
            continue;
        };
        if signature.entry_kind.is_some() && signature.semantics.is_some() {
            diagnostics.push(
                Diagnostic::error(
                    V14,
                    "entry records do not declare a function semantics override",
                )
                .with_primary_span(binary_span(0)),
            );
        }
        let mode = signature.semantics.unwrap_or(module_mode);
        if mode != SemanticsMode::Defined {
            continue;
        }
        for instruction in body
            .blocks
            .iter()
            .filter_map(|block| block.instructions.as_deref())
            .flatten()
        {
            if let Some(form) = permissive_form(instruction) {
                diagnostics.push(
                    Diagnostic::error(
                        V14,
                        format!("{form} is only permitted in a permissive function"),
                    )
                    .with_primary_span(binary_span(instruction.offset)),
                );
            }
        }
    }
    diagnostics
}

fn permissive_form(instruction: &BinaryInstruction) -> Option<&'static str> {
    let opcode = instruction.opcode;
    let modifiers = instruction.modifiers;
    if opcode == 0x005 {
        return Some("const.poison");
    }
    if opcode == 0x243 {
        return Some("assume");
    }
    if opcode == 0x1e8 && selector(modifiers) == 1 {
        return Some("unreachable [ub]");
    }
    if opcode == 0x02c && modifiers & (1 << 4) != 0 {
        return Some("or [disjoint]");
    }
    if opcode == 0x148 && modifiers & (1 << 5) != 0 {
        return Some("ptr.offset [inbounds]");
    }
    if opcode == 0x14a && modifiers & (1 << 31) != 0 {
        return Some("mem.copy [nooverlap]");
    }
    if opcode == 0x164 && modifiers & (1 << 30) != 0 {
        return Some("heap.alloc.raw [uninit]");
    }
    if modifiers & 0b11 == 0b11 && matches!(opcode, 0x020..=0x022 | 0x02f | 0x031) {
        return Some("[poison]");
    }
    if modifiers & (1 << 3) != 0 && matches!(opcode, 0x027 | 0x028 | 0x032 | 0x033) {
        return Some("[exact]");
    }
    if modifiers & (1 << 47) != 0 && matches!(opcode, 0x03e | 0x03f) {
        return Some("[poison]");
    }
    if matches!(opcode, 0x0a7 | 0x0a8) && selector(modifiers) == 2 {
        return Some("[poison]");
    }
    if (0x060..=0x09f).contains(&opcode) && modifiers & ((1 << 6) | (1 << 7)) != 0 {
        return Some("nnan/ninf fast-math flags");
    }
    None
}

fn selector(modifiers: u64) -> u64 {
    (modifiers >> 48) & 0xf
}

fn binary_span(offset: u64) -> Span {
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    Span::new(start, start)
}
