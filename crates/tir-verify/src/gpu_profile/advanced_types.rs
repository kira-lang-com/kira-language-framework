use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::parse_type;
use tir_format::text::{Token, TokenKind};
use tir_types::{AddressSpace, MatrixScope, Type};

use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

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
        "node.output.allocate" => {
            if operands.first().is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "node output selector must be an integer",
                );
            }
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
            if let Some(coordinates) = operands.get(1) {
                verify_coordinates(instruction, coordinates, rank, diagnostics);
            }
        }
        "tensor.store" => {
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
            if !instruction.tokens.iter().any(|token| {
                matches!(&token.kind, TokenKind::Atom(value) if matches!(value.as_str(), "extent" | "stride"))
            }) {
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
    match instruction.opcode.as_str() {
        "coop.fill" => {
            let target = explicit_type_before_comma(source, instruction);
            if let Some(element) = target.as_ref().and_then(cooperative_element)
                && operands.first().is_some_and(|ty| ty != element)
            {
                error(
                    diagnostics,
                    instruction,
                    format!("coop.fill value must have type {element}"),
                );
            }
        }
        "coop.load" => {
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
            }
        }
        "coop.store" => {
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
        }
        "coop.mma" => verify_mma(instruction, operands, diagnostics),
        "coop.convert" => verify_coop_convert(instruction, source, operands, diagnostics),
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
            if operands.first().and_then(cooperative_element).is_none() {
                error(
                    diagnostics,
                    instruction,
                    "coop.reduce requires a cooperative matrix",
                );
            }
        }
        _ => {}
    }
}

fn verify_coop_convert(
    instruction: &TextInstruction,
    source: &str,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((input_scope, input_rows, input_columns, _)) = operands.first().and_then(matrix_type)
    else {
        error(
            diagnostics,
            instruction,
            "coop.convert requires a cooperative matrix",
        );
        return;
    };
    let Some(target) = type_after_atom(source, instruction, "to") else {
        error(
            diagnostics,
            instruction,
            "coop.convert requires a target matrix type",
        );
        return;
    };
    let Some((target_scope, target_rows, target_columns, _)) = matrix_type(&target) else {
        error(
            diagnostics,
            instruction,
            "coop.convert target must be a cooperative matrix",
        );
        return;
    };
    if (input_scope, input_rows, input_columns) != (target_scope, target_rows, target_columns) {
        error(
            diagnostics,
            instruction,
            "coop.convert must preserve matrix scope and dimensions",
        );
    }
}

fn verify_mma(instruction: &TextInstruction, operands: &[Type], diagnostics: &mut Vec<Diagnostic>) {
    let Some((a_scope, a_rows, a_columns, a_use)) = operands.first().and_then(matrix_type) else {
        return;
    };
    let Some((b_scope, b_rows, b_columns, b_use)) = operands.get(1).and_then(matrix_type) else {
        return;
    };
    let Some((c_scope, c_rows, c_columns, c_use)) = operands.get(2).and_then(matrix_type) else {
        return;
    };
    if a_use != "a" || b_use != "b" || c_use != "acc" {
        error(
            diagnostics,
            instruction,
            "coop.mma operands must use a, b, and acc roles",
        );
    }
    if a_scope != b_scope || a_scope != c_scope {
        error(
            diagnostics,
            instruction,
            "coop.mma operands must have the same scope",
        );
    }
    if a_columns != b_rows || a_rows != c_rows || b_columns != c_columns {
        error(
            diagnostics,
            instruction,
            "coop.mma matrix dimensions are incompatible",
        );
    }
}

fn require_pointer_space(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    space: AddressSpace,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands
        .get(index)
        .is_some_and(|ty| ty != &Type::Pointer(space))
    {
        error(
            diagnostics,
            instruction,
            format!(
                "{} operand {} must have ptr<{}> type",
                instruction.opcode,
                index + 1,
                space.name()
            ),
        );
    }
}

fn require_memory_pointer(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.get(index).is_some_and(|ty| {
        !matches!(
            ty,
            Type::Pointer(AddressSpace::Device | AddressSpace::Threadgroup)
        )
    }) {
        error(
            diagnostics,
            instruction,
            "cooperative memory operand must be a device or threadgroup pointer",
        );
    }
}

fn tensor_type(ty: &Type) -> Option<(u32, &Type)> {
    if let Type::Tensor { count, element } = ty {
        Some((*count, element))
    } else {
        None
    }
}

fn tensor_element(ty: &Type) -> Option<&Type> {
    tensor_type(ty).map(|(_, element)| element)
}

fn cooperative_element(ty: &Type) -> Option<&Type> {
    if let Type::CooperativeMatrix { element, .. } = ty {
        Some(element)
    } else {
        None
    }
}

fn matrix_type(ty: &Type) -> Option<(MatrixScope, u32, u32, &'static str)> {
    if let Type::CooperativeMatrix {
        scope,
        rows,
        columns,
        use_kind,
        ..
    } = ty
    {
        Some((*scope, *rows, *columns, use_kind.name()))
    } else {
        None
    }
}

fn verify_coordinates(
    instruction: &TextInstruction,
    ty: &Type,
    rank: u32,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let valid = match ty {
        Type::Integer(_) => rank == 1,
        Type::FixedVector { lanes, element } => {
            *lanes == rank && matches!(element.as_ref(), Type::Integer(_))
        }
        _ => false,
    };
    if !valid {
        error(
            diagnostics,
            instruction,
            format!(
                "{} coordinates must contain {rank} integers",
                instruction.opcode
            ),
        );
    }
}

fn integer_immediate_after_first_comma(instruction: &TextInstruction) -> Option<u32> {
    let start = text_instruction_operands_start(&instruction.tokens);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    u32::try_from(tir_format::numeric_literals::parse_unsigned_integer(atom(
        instruction.tokens.get(comma + 1)?,
    )?)?)
    .ok()
}

fn explicit_type_before_comma(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(&instruction.tokens);
    let end = top_level_commas(tokens, start)
        .first()
        .copied()
        .unwrap_or(tokens.len());
    type_from_tokens(source, tokens, start, end)
}

fn type_after_atom(source: &str, instruction: &TextInstruction, marker: &str) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = tokens
        .iter()
        .position(|token| atom(token) == Some(marker))?
        + 1;
    type_from_tokens(source, tokens, start, tokens.len())
}

fn top_level_commas(tokens: &[Token], start: usize) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => commas.push(index),
                _ => {}
            }
        }
    }
    commas
}

fn type_from_tokens(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    parse_type(source.get(first.span.start..last.span.end)?).ok()
}

fn is_integer_type(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn is_index_value(ty: &Type) -> bool {
    match ty {
        Type::Integer(32) => true,
        Type::FixedVector {
            lanes: 2 | 3,
            element,
        } => element.as_ref() == &Type::Integer(32),
        _ => false,
    }
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(instruction.span));
}
