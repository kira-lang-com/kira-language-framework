use tir_diag::Diagnostic;
use tir_format::TextInstruction;
use tir_types::{AddressSpace, Type};

use crate::calls::opcode_end;

mod cooperative;
mod helpers;
use cooperative::{verify_coop_convert, verify_coop_reduce, verify_mma};
use helpers::{
    atom, cooperative_element, error, explicit_type_before_comma, has_modifier, has_query_field,
    integer_immediate_after_first_comma, is_index_value, is_integer_type, require_memory_pointer,
    require_pointer_space, tensor_element, tensor_query_field, tensor_type, type_after_atom,
    verify_commas, verify_coordinates, verify_matrix_layout, verify_words, words_after_opcode,
    words_for_value_operand,
};

pub(super) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match instruction.opcode.as_str() {
        "object.dispatch_mesh" | "mesh.set_counts" => {
            if operands.iter().any(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} operands must be integers", instruction.opcode),
                );
            }
        }
        "mesh.write.vertex" | "mesh.write.primitive" => {
            if operands.first().is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "mesh output index must be an integer",
                );
            }
        }
        "mesh.write.indices" => {
            if operands.first().is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "mesh.write.indices offset must be an integer",
                );
            }
            if operands.get(1).is_some_and(|ty| !is_index_value(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "mesh.write.indices requires an i32 index or vector of i32 indices",
                );
            }
        }
        "tensor.load" | "tensor.store" | "tensor.query" => {
            verify_tensor(instruction, operands, &mut diagnostics);
        }
        "coop.fill" | "coop.load" | "coop.store" | "coop.mma" | "coop.convert" | "coop.length"
        | "coop.extract" | "coop.insert" | "coop.reduce" => {
            verify_cooperative(instruction, source, operands, &mut diagnostics);
        }
        "node.input.addr" => {
            if operands.first().is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "node record index must be an integer",
                );
            }
        }
        "node.output.addr" => {
            require_pointer_space(
                instruction,
                operands,
                0,
                AddressSpace::NodeRecord,
                &mut diagnostics,
            );
            if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "node record index must be an integer",
                );
            }
        }
        "tile.imageblock.addr" => {
            if operands.first().is_some_and(|ty| {
                ty != &Type::FixedVector {
                    lanes: 2,
                    element: Box::new(Type::Integer(32)),
                }
            }) {
                error(
                    &mut diagnostics,
                    instruction,
                    "tile.imageblock.addr position must have type vec<2 x i32>",
                );
            }
        }
        "node.output.allocate" => {
            if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "node output count must be an integer",
                );
            }
        }
        "node.output.commit" => require_pointer_space(
            instruction,
            operands,
            0,
            AddressSpace::NodeRecord,
            &mut diagnostics,
        ),
        _ => {}
    }
    diagnostics
}

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Type> {
    match instruction.opcode.as_str() {
        "coop.fill" | "coop.load" => explicit_type_before_comma(source, instruction)
            .into_iter()
            .collect(),
        "coop.mma" => operands.get(2).cloned().into_iter().collect(),
        "coop.convert" => type_after_atom(source, instruction, "to")
            .into_iter()
            .collect(),
        "coop.length" => vec![Type::Integer(32)],
        "coop.extract" => operands
            .first()
            .and_then(cooperative_element)
            .cloned()
            .into_iter()
            .collect(),
        "coop.insert" => operands.first().cloned().into_iter().collect(),
        "coop.reduce" => operands.first().cloned().into_iter().collect(),
        "tensor.load" => operands
            .first()
            .and_then(tensor_element)
            .cloned()
            .into_iter()
            .collect(),
        "tensor.query" => vec![Type::Integer(64)],
        "frag.imageblock.addr" | "tile.imageblock.addr" => {
            vec![Type::Pointer(AddressSpace::Tile)]
        }
        "node.input.addr" | "node.output.allocate" | "node.output.addr" => {
            vec![Type::Pointer(AddressSpace::NodeRecord)]
        }
        "node.input.count" => vec![Type::Integer(32)],
        "node.finished_cross_group" => vec![Type::Integer(1)],
        _ => Vec::new(),
    }
}

fn verify_tensor(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expected_commas = match instruction.opcode.as_str() {
        "tensor.load" | "tensor.query" => 1,
        "tensor.store" => 2,
        _ => 0,
    };
    verify_commas(instruction, expected_commas, diagnostics);
    let Some((rank, element)) = operands.first().and_then(tensor_type) else {
        if operands.first().is_some() {
            error(
                diagnostics,
                instruction,
                "tensor instruction requires a tensor handle",
            );
        }
        return;
    };
    match instruction.opcode.as_str() {
        "tensor.load" => {
            verify_words(
                instruction,
                &words_after_opcode(instruction),
                &[],
                "tensor.load",
                diagnostics,
            );
            if let Some(coordinates) = operands.get(1) {
                verify_coordinates(instruction, coordinates, rank, diagnostics);
            }
        }
        "tensor.store" => {
            verify_words(
                instruction,
                &words_after_opcode(instruction),
                &[],
                "tensor.store",
                diagnostics,
            );
            if let Some(coordinates) = operands.get(1) {
                verify_coordinates(instruction, coordinates, rank, diagnostics);
            }
            if operands.get(2).is_some_and(|ty| ty != element) {
                error(
                    diagnostics,
                    instruction,
                    format!("tensor.store value must have type {element}"),
                );
            }
        }
        "tensor.query" => {
            let words = words_after_opcode(instruction);
            let dimensions = words
                .iter()
                .filter(|word| tir_format::numeric_literals::parse_unsigned_integer(word).is_some())
                .count();
            if dimensions != 1 {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query requires exactly one dimension literal",
                );
            }
            if words.iter().any(|word| {
                tir_format::numeric_literals::parse_unsigned_integer(word).is_none()
                    && !matches!(word.as_str(), "extent" | "stride")
            }) {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query contains an unsupported field",
                );
            }
            if ["extent", "stride"]
                .into_iter()
                .any(|field| words.iter().filter(|word| word.as_str() == field).count() > 1)
            {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query repeats its extent or stride field",
                );
            }
            let Some(dimension) = integer_immediate_after_first_comma(instruction) else {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query dimension must be an integer",
                );
                return;
            };
            if dimension >= rank {
                error(
                    diagnostics,
                    instruction,
                    format!("tensor.query dimension must be less than rank {rank}"),
                );
            }
            let selected_fields = usize::from(has_query_field(instruction, "extent"))
                + usize::from(has_query_field(instruction, "stride"));
            if selected_fields > 1 {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query must select at most one of extent or stride",
                );
            }
            if tensor_query_field(instruction)
                .is_some_and(|field| !matches!(field, "extent" | "stride"))
            {
                error(
                    diagnostics,
                    instruction,
                    "tensor.query must select extent or stride",
                );
            }
        }
        _ => {}
    }
}

fn verify_cooperative(
    instruction: &TextInstruction,
    source: &str,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expected_commas = match instruction.opcode.as_str() {
        "coop.fill" | "coop.extract" | "coop.reduce" => 1,
        "coop.load" | "coop.store" | "coop.mma" | "coop.insert" => 2,
        "coop.convert" | "coop.length" => 0,
        _ => 0,
    };
    verify_commas(instruction, expected_commas, diagnostics);
    match instruction.opcode.as_str() {
        "coop.fill" => {
            let words = words_for_value_operand(instruction, 0, true, false).unwrap_or_default();
            verify_words(instruction, &words, &[], "coop.fill", diagnostics);
            let target = explicit_type_before_comma(source, instruction);
            if let Some(element) = target.as_ref().and_then(cooperative_element)
                && operands.first().is_some_and(|ty| ty != element)
            {
                error(
                    diagnostics,
                    instruction,
                    format!("coop.fill value must have type {element}"),
                );
            } else if target.as_ref().and_then(cooperative_element).is_none() {
                error(
                    diagnostics,
                    instruction,
                    "coop.fill requires a cooperative matrix type",
                );
            }
        }
        "coop.load" => {
            let tensor_form = operands.first().and_then(tensor_type).is_some();
            let address_words =
                words_for_value_operand(instruction, 0, true, !tensor_form).unwrap_or_default();
            let stride_words =
                words_for_value_operand(instruction, 1, true, false).unwrap_or_default();
            verify_words(
                instruction,
                &address_words,
                &[],
                "coop.load address",
                diagnostics,
            );
            verify_words(
                instruction,
                &stride_words,
                &["row_major", "col_major"],
                "coop.load layout",
                diagnostics,
            );
            let target = explicit_type_before_comma(source, instruction);
            let Some(element) = target.as_ref().and_then(cooperative_element) else {
                error(
                    diagnostics,
                    instruction,
                    "coop.load requires a cooperative matrix result type",
                );
                return;
            };
            if let Some((rank, tensor_element)) = operands.first().and_then(tensor_type) {
                if has_modifier(instruction, "row_major") || has_modifier(instruction, "col_major")
                {
                    error(
                        diagnostics,
                        instruction,
                        "tensor-form coop.load does not take a matrix layout",
                    );
                }
                if tensor_element != element {
                    error(
                        diagnostics,
                        instruction,
                        format!("coop.load tensor element type must be {element}"),
                    );
                }
                if let Some(coordinates) = operands.get(1) {
                    verify_coordinates(instruction, coordinates, rank, diagnostics);
                }
            } else {
                require_memory_pointer(instruction, operands, 0, diagnostics);
                if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
                    error(
                        diagnostics,
                        instruction,
                        "coop.load stride must be an integer",
                    );
                }
                verify_matrix_layout(instruction, diagnostics);
            }
        }
        "coop.store" => {
            let matrix_words =
                words_for_value_operand(instruction, 0, false, false).unwrap_or_default();
            let address_words =
                words_for_value_operand(instruction, 1, false, true).unwrap_or_default();
            let stride_words =
                words_for_value_operand(instruction, 2, false, false).unwrap_or_default();
            verify_words(
                instruction,
                &matrix_words,
                &[],
                "coop.store matrix",
                diagnostics,
            );
            verify_words(
                instruction,
                &address_words,
                &[],
                "coop.store address",
                diagnostics,
            );
            verify_words(
                instruction,
                &stride_words,
                &["row_major", "col_major"],
                "coop.store layout",
                diagnostics,
            );
            if operands.first().and_then(cooperative_element).is_none() {
                error(
                    diagnostics,
                    instruction,
                    "coop.store requires a cooperative matrix",
                );
            }
            require_memory_pointer(instruction, operands, 1, diagnostics);
            if operands.get(2).is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    diagnostics,
                    instruction,
                    "coop.store stride must be an integer",
                );
            }
            verify_matrix_layout(instruction, diagnostics);
        }
        "coop.mma" => {
            let words = words_after_opcode(instruction);
            verify_words(
                instruction,
                &words,
                &["unsigned", "saturate"],
                "coop.mma",
                diagnostics,
            );
            for modifier in ["unsigned", "saturate"] {
                if words
                    .iter()
                    .filter(|word| word.as_str() == modifier)
                    .count()
                    > 1
                {
                    error(
                        diagnostics,
                        instruction,
                        format!("coop.mma repeats the {modifier} modifier"),
                    );
                }
            }
            verify_mma(instruction, operands, diagnostics);
        }
        "coop.convert" => {
            let start = opcode_end(instruction);
            if !instruction
                .tokens
                .get(start + 2)
                .and_then(atom)
                .is_some_and(|atom| atom == "to")
            {
                error(
                    diagnostics,
                    instruction,
                    "coop.convert syntax is `%matrix to matrix_type`",
                );
            }
            verify_coop_convert(instruction, source, operands, diagnostics);
        }
        "coop.length" => {
            if explicit_type_before_comma(source, instruction)
                .as_ref()
                .and_then(cooperative_element)
                .is_none()
            {
                error(
                    diagnostics,
                    instruction,
                    "coop.length requires a cooperative matrix type",
                );
            }
        }
        "coop.extract" => {
            verify_words(
                instruction,
                &words_after_opcode(instruction),
                &[],
                "coop.extract",
                diagnostics,
            );
            if operands.first().and_then(cooperative_element).is_none() {
                error(
                    diagnostics,
                    instruction,
                    "coop.extract requires a cooperative matrix",
                );
            }
            if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    diagnostics,
                    instruction,
                    "coop.extract index must be an integer",
                );
            }
        }
        "coop.insert" => {
            verify_words(
                instruction,
                &words_after_opcode(instruction),
                &[],
                "coop.insert",
                diagnostics,
            );
            if let Some(element) = operands.first().and_then(cooperative_element) {
                if operands.get(2).is_some_and(|ty| ty != element) {
                    error(
                        diagnostics,
                        instruction,
                        format!("coop.insert value must have type {element}"),
                    );
                }
            } else {
                error(
                    diagnostics,
                    instruction,
                    "coop.insert requires a cooperative matrix",
                );
            }
            if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    diagnostics,
                    instruction,
                    "coop.insert index must be an integer",
                );
            }
        }
        "coop.reduce" => {
            verify_coop_reduce(instruction, operands, diagnostics);
        }
        _ => {}
    }
}
