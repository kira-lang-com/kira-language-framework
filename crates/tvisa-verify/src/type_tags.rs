use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::TokenKind;
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TextCallingConvention, TextSignature,
    TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_text_type_tag(
    instruction: &TextInstruction,
    comma_index: usize,
    allow_foreign_call: bool,
) -> Vec<Diagnostic> {
    let tag = text_type_tag(instruction, comma_index);
    if tag.is_some_and(tvisa_format::is_valid_type_tag) || allow_foreign_call && tag == Some(0) {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                VFY3,
                "code pointer operation or indirect call has an invalid or forbidden type tag",
            )
            .with_primary_span(instruction.span),
        ]
    }
}

pub(super) fn verify_text_indirect_type_tag(
    instruction: &TextInstruction,
    signature: &TextSignature,
) -> Vec<Diagnostic> {
    verify_text_type_tag(
        instruction,
        0,
        signature.convention == TextCallingConvention::C,
    )
}

pub(super) fn text_type_tag(instruction: &TextInstruction, comma_index: usize) -> Option<u32> {
    let operands = &instruction.tokens[crate::calls::opcode_end(instruction)..];
    operands
        .iter()
        .enumerate()
        .filter(|(_, token)| matches!(&token.kind, TokenKind::Punctuation(',')))
        .nth(comma_index)
        .and_then(|(comma, _)| operands.get(comma + 1))
        .and_then(|token| match &token.kind {
            TokenKind::Atom(value) => tir_format::numeric_literals::parse_unsigned_integer(value),
            _ => None,
        })
        .and_then(|tag| u32::try_from(tag).ok())
}

pub(super) fn verify_binary_function(
    body: &BinaryFunctionBody,
    types: &TypeSection,
) -> Vec<Diagnostic> {
    body.blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| has_type_tag(*instruction))
        .filter_map(|instruction| verify_binary_instruction(instruction, types))
        .collect()
}

fn has_type_tag(instruction: &BinaryInstruction) -> bool {
    matches!(instruction.opcode, 0x1a1 | 0x708 | 0x709 | 0x70b)
        || (matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf == 1)
}

fn verify_binary_instruction(
    instruction: &BinaryInstruction,
    types: &TypeSection,
) -> Option<Diagnostic> {
    let tag = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::TypeTag(tag) => Some(*tag),
            _ => None,
        })?;
    let foreign_call = tag == 0
        && is_indirect_call(instruction)
        && instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => usize::try_from(*index).ok(),
                _ => None,
            })
            .and_then(|index| types.signatures.get(index))
            .is_some_and(|signature| signature.convention == 1);
    (!tvisa_format::is_valid_type_tag(tag) && !foreign_call).then(|| {
        Diagnostic::error(
            VFY3,
            format!("code pointer operation or indirect call has forbidden type tag 0x{tag:08X}"),
        )
    })
}

fn is_indirect_call(instruction: &BinaryInstruction) -> bool {
    instruction.opcode == 0x1a1
        || matches!(instruction.opcode, 0x1aa | 0x1e7) && (instruction.modifiers >> 48) & 0xf == 1
}
