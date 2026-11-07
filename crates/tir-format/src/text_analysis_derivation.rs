//! Derive the narrow CODE-known ANALYSIS trust classification for text assembly.
//! This does not grant managed ownership semantics to raw pointer instructions.

use crate::text::TokenKind;
use crate::text_signature_assembly::ParsedFunction;
use crate::{
    BinaryAnalysisSummary, BinaryCodeTable, BinaryFunctionAnalysis, BinaryOperand,
    BinarySignatureTable, BinaryTypeTable, TextModule, TextParseError,
};
use std::collections::{BTreeSet, HashMap};
use tir_types::Type;

pub(crate) fn derive(function: &ParsedFunction) -> BinaryFunctionAnalysis {
    let mut reason_mask = 0u8;
    let mut unmanaged = false;
    if let Some(body) = &function.body {
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            match instruction.opcode.as_str() {
                "asm" | "asm.goto" => reason_mask |= 1 << 0,
                "raw.load" | "raw.store" | "ptr.offset" | "ptr.diff" | "mem.copy" | "mem.move"
                | "mem.set" | "into.raw" | "from.raw" | "heap.alloc.raw" | "heap.realloc.raw"
                | "heap.free.raw" => {
                    reason_mask |= 1 << 1;
                }
                "addr.toptr" | "ptr.toaddr"
                    if function.record.target != crate::types::BinaryTarget::Gpu =>
                {
                    reason_mask |= 1 << 1;
                }
                "inttoptr" => reason_mask |= 1 << 2,
                "call.foreign" | "call.variadic" | "va.start" | "va.end" | "va.arg" | "va.copy" => {
                    reason_mask |= 1 << 3
                }
                "setjmp" | "longjmp" => reason_mask |= 1 << 4,
                "elem.addr"
                    if instruction.tokens.windows(3).any(|tokens| {
                        matches!(
                            (&tokens[0].kind, &tokens[1].kind, &tokens[2].kind),
                            (
                                TokenKind::Punctuation('['),
                                TokenKind::Atom(word),
                                TokenKind::Punctuation(']')
                            ) if word == "unchecked"
                        )
                    }) =>
                {
                    reason_mask |= 1 << 5
                }
                "foreign.addr" | "foreign.data" | "foreign.entry" | "kernel.percpu.addr" => {
                    reason_mask |= 1 << 7
                }
                "mmio.load" | "mmio.store" | "fence.io" => reason_mask |= 1 << 7,
                "target.intrinsic" => unmanaged = true,
                _ => {}
            }
        }
    }
    BinaryFunctionAnalysis {
        trust_class: u8::from(reason_mask != 0 || unmanaged) * 2,
        reason_mask,
    }
}

pub(crate) fn reconstructible_from_code(
    analysis: &BinaryAnalysisSummary,
    signature_count: usize,
    code: &BinaryCodeTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    module_path: &str,
) -> bool {
    if analysis.functions.len() != signature_count
        || !analysis.sandbox_regions.is_empty()
        || !analysis.assembly_regions.is_empty()
    {
        return false;
    }
    if indirect_typeids_from_binary(code, types, signatures, module_path)
        .is_none_or(|expected| expected != analysis.indirect_call_typeids)
    {
        return false;
    }
    let mut expected = vec![
        BinaryFunctionAnalysis {
            trust_class: 0,
            reason_mask: 0
        };
        signature_count
    ];
    let mut visited = vec![false; signature_count];
    for body in &code.functions {
        let Ok(index) = usize::try_from(body.function_index) else {
            return false;
        };
        if index >= signature_count || visited[index] {
            return false;
        }
        visited[index] = true;
        let mut reasons = 0u8;
        for block in &body.blocks {
            let Some(instructions) = &block.instructions else {
                return false;
            };
            for instruction in instructions {
                match instruction.opcode {
                    0x123 if instruction.modifiers & (1 << 33) != 0 => reasons |= 1 << 5,
                    0x127 | 0x128
                        if signatures.records.get(index).is_some_and(|signature| {
                            signature.target != crate::types::BinaryTarget::Gpu
                        }) =>
                    {
                        reasons |= 1 << 1;
                    }
                    0x146..=0x14c | 0x164..=0x166 | 0x185..=0x186 => reasons |= 1 << 1,
                    0x0aa => reasons |= 1 << 2,
                    0x270..=0x273 => reasons |= 1 << 7,
                    0x210..=0x213 => reasons |= 1 << 3,
                    0x214..=0x215 => reasons |= 1 << 4,
                    _ => {}
                }
            }
        }
        expected[index] = BinaryFunctionAnalysis {
            trust_class: u8::from(reasons != 0) * 2,
            reason_mask: reasons,
        };
    }
    expected == analysis.functions
}

pub(crate) fn indirect_typeids_from_text(
    module: &TextModule,
    functions: &[ParsedFunction],
) -> Result<Vec<u64>, TextParseError> {
    let mut ids = BTreeSet::new();
    for function in functions {
        let Some(body) = &function.body else {
            continue;
        };
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction.opcode != "call.indirect" {
                continue;
            }
            let call = crate::text_indirect_call::parse(
                instruction,
                module.item_text(function.item),
                function.item,
            )?;
            let id =
                typeid(&call.function_type, &module.header.path).ok_or_else(|| TextParseError {
                    span: instruction.span,
                    message: "indirect call function type cannot be canonicalized to a typeid"
                        .to_owned(),
                })?;
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().collect())
}

fn indirect_typeids_from_binary(
    code: &BinaryCodeTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    module_path: &str,
) -> Option<Vec<u64>> {
    let mut ids = BTreeSet::new();
    for body in &code.functions {
        for block in &body.blocks {
            let instructions = block.instructions.as_deref()?;
            for instruction in instructions {
                if instruction.opcode != 0x1a1 {
                    continue;
                }
                let index = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Type(index) => Some(*index),
                        _ => None,
                    })?;
                let text =
                    crate::text_type_disassembly::format_type_reference(types, index, signatures)
                        .ok()?;
                let ty = crate::parse_type(&text).ok()?;
                ids.insert(typeid(&ty, module_path)?);
            }
        }
    }
    Some(ids.into_iter().collect())
}

fn typeid(ty: &Type, module_path: &str) -> Option<u64> {
    ty.type_id_in(module_path, &HashMap::new())
        .ok()
        .map(|id| id.as_u64())
}
