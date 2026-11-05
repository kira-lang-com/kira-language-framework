use std::collections::HashMap;

use tir_diag::Span;
use tir_types::{FunctionType, Type};

use crate::text::TokenKind;
use crate::text_function_constant_assembly::{ParsedEntryConstant, parse_entry_constant};
use crate::text_integer_operation_assembly::{integer_operation_info, parse_integer_operation};
use crate::text_select_assembly::parse_select;
use crate::types::{BinaryBlock, BinaryBlockParameter, BinaryFunctionBody, BinaryFunctionType};
use crate::types::{BinaryType, BinaryTypeTable};
use crate::{FunctionBody, TextItem, TextParseError};

pub(crate) fn validate(
    body: &FunctionBody,
    signature: &FunctionType,
    source: &str,
    item: TextItem,
) -> Result<(), TextParseError> {
    let block = &body.blocks[0];
    let mut values = block
        .parameters
        .iter()
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect::<HashMap<_, _>>();
    if values.len() != block.parameters.len() {
        return Err(error(item, block.span, "duplicate entry block parameter"));
    }
    let Some((last, instructions)) = block.instructions.split_last() else {
        return Err(error(item, block.span, "function body has no terminator"));
    };
    validate_operations(instructions, &mut values, source, item)?;
    if last.opcode != "return" || !last.results.is_empty() {
        return Err(error(
            item,
            last.span,
            "straight-line function must end in return",
        ));
    }
    match signature.result.as_ref() {
        Type::Unit if last.tokens.len() == 1 => Ok(()),
        Type::Unit => Err(error(
            item,
            last.span,
            "unit return must not have an operand",
        )),
        result => {
            let [_, percent, name] = last.tokens.as_slice() else {
                return Err(error(item, last.span, "return requires one SSA operand"));
            };
            if !matches!(&percent.kind, TokenKind::Punctuation('%')) {
                return Err(error(item, last.span, "return requires one SSA operand"));
            }
            let TokenKind::Atom(name) = &name.kind else {
                return Err(error(item, last.span, "invalid return operand"));
            };
            if values.get(name) != Some(result) {
                return Err(error(
                    item,
                    last.span,
                    "return operand has the wrong type or is undefined",
                ));
            }
            Ok(())
        }
    }
}

pub(crate) fn validate_operations(
    instructions: &[crate::TextInstruction],
    values: &mut HashMap<String, Type>,
    source: &str,
    item: TextItem,
) -> Result<(), TextParseError> {
    for instruction in instructions {
        let (name, ty) = if integer_operation_info(&instruction.opcode).is_some() {
            let parsed = parse_integer_operation(instruction, item)?;
            let types = parsed
                .operands
                .iter()
                .map(|name| {
                    values.get(name).ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "integer operation references an undefined SSA value",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if parsed.opcode == "icmp" {
                if !matches!(types[0], Type::Integer(_) | Type::Pointer(_))
                    || types.iter().any(|ty| *ty != types[0])
                    || (matches!(types[0], Type::Pointer(_))
                        && matches!(parsed.modifiers >> 48, 2..=5))
                {
                    return Err(error(
                        item,
                        instruction.span,
                        "icmp operands must have the same integer or pointer type",
                    ));
                }
                (parsed.name, Type::Integer(1))
            } else {
                if !matches!(types[0], Type::Integer(_)) || types.iter().any(|ty| *ty != types[0]) {
                    return Err(error(
                        item,
                        instruction.span,
                        "integer operation operands must have the same integer type",
                    ));
                }
                (parsed.name, (*types[0]).clone())
            }
        } else if matches!(instruction.opcode.as_str(), "const.int" | "const.bool") {
            let constant = parse_entry_constant(instruction, source, item)?;
            (constant.name().to_owned(), constant.ty().clone())
        } else if crate::text_memory_assembly::handles(&instruction.opcode) {
            let parsed = crate::text_memory_assembly::parse(instruction, source, item)?;
            let result_ty = parsed.result_type(values, item, instruction.span)?;
            let Some(ty) = result_ty else {
                continue;
            };
            let name = instruction
                .results
                .first()
                .ok_or_else(|| error(item, instruction.span, "memory result is missing"))?;
            (name.clone(), ty)
        } else if matches!(
            instruction.opcode.as_str(),
            "copy" | "move" | "destroy" | "borrow.begin" | "borrow.end"
        ) {
            let parsed = crate::text_ownership_assembly::parse(instruction, item)?;
            let operand_type = values
                .get(&parsed.operand)
                .ok_or_else(|| error(item, instruction.span, "ownership operand is undefined"))?
                .clone();
            let Some(name) = parsed.result else {
                continue;
            };
            (name, operand_type)
        } else if instruction.opcode == "select" {
            let parsed = parse_select(instruction, item)?;
            let condition = values
                .get(&parsed.condition)
                .ok_or_else(|| error(item, instruction.span, "select condition is undefined"))?;
            let when_true = values
                .get(&parsed.when_true)
                .ok_or_else(|| error(item, instruction.span, "select true operand is undefined"))?;
            let when_false = values.get(&parsed.when_false).ok_or_else(|| {
                error(item, instruction.span, "select false operand is undefined")
            })?;
            if condition != &Type::Integer(1) || when_true != when_false {
                return Err(error(
                    item,
                    instruction.span,
                    "select requires an i1 condition and matching value types",
                ));
            }
            (parsed.name, when_true.clone())
        } else {
            return Err(error(
                item,
                instruction.span,
                "straight-line assembly supports integer operations, integer/boolean constants and select",
            ));
        };
        if values.insert(name, ty).is_some() {
            return Err(error(
                item,
                instruction.span,
                "SSA value is defined more than once",
            ));
        }
    }
    Ok(())
}

pub(crate) fn encode(
    body: &FunctionBody,
    function_index: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    source: &str,
    item: TextItem,
) -> Result<BinaryFunctionBody, TextParseError> {
    let block = &body.blocks[0];
    let mut values: Vec<&str> = block
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    let mut bytes = Vec::new();
    encode_operations(
        &block.instructions[..block.instructions.len() - 1],
        &mut values,
        types,
        source,
        item,
        &mut bytes,
    )?;
    let terminator = block.instructions.last().expect("validated return");
    write(0x1e5, &mut bytes, item, terminator.span)?;
    write(0, &mut bytes, item, terminator.span)?;
    write(0, &mut bytes, item, terminator.span)?;
    if let [_, _, name] = terminator.tokens.as_slice() {
        let TokenKind::Atom(name) = &name.kind else {
            return Err(error(item, terminator.span, "invalid return operand"));
        };
        let position = values
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| error(item, terminator.span, "undefined return value"))?;
        write(
            (values.len() - position) as u64,
            &mut bytes,
            item,
            terminator.span,
        )?;
    }
    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size: 0,
        block_count: 1,
        blocks: vec![BinaryBlock {
            cold: false,
            loop_merge: None,
            loop_continue: None,
            parameters: function
                .parameters
                .iter()
                .map(|parameter| BinaryBlockParameter {
                    convention: parameter.convention,
                    context: parameter.context,
                    ty: parameter.ty,
                })
                .collect(),
            instruction_count: block.instructions.len() as u64,
            instruction_bytes: bytes,
            instructions: None,
        }],
    })
}

pub(crate) fn encode_operations<'a>(
    instructions: &'a [crate::TextInstruction],
    values: &mut Vec<&'a str>,
    types: &BinaryTypeTable,
    source: &str,
    item: TextItem,
    mut bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    for instruction in instructions {
        if let Some((opcode, _)) = integer_operation_info(&instruction.opcode) {
            let parsed = parse_integer_operation(instruction, item)?;
            write(opcode, &mut bytes, item, instruction.span)?;
            write(parsed.modifiers, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            for operand in &parsed.operands {
                let position = values
                    .iter()
                    .position(|name| name == operand)
                    .ok_or_else(|| error(item, instruction.span, "undefined SSA operand"))?;
                write(
                    (values.len() - position) as u64,
                    &mut bytes,
                    item,
                    instruction.span,
                )?;
            }
        } else if matches!(instruction.opcode.as_str(), "const.int" | "const.bool") {
            let constant = parse_entry_constant(instruction, source, item)?;
            match constant {
                ParsedEntryConstant::Integer(value) => {
                    let Type::Integer(width) = value.ty else {
                        return Err(error(
                            item,
                            instruction.span,
                            "const.int must have an integer type",
                        ));
                    };
                    let ty = types.entries.iter().position(|ty| matches!(ty, BinaryType::Integer(bits) if *bits == u64::from(width)))
                        .ok_or_else(|| error(item, instruction.span, "const.int type is not interned"))?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(ty as u64, &mut bytes, item, instruction.span)?;
                    crate::leb::write_integer_bits(value.value, width, &mut bytes).map_err(
                        |error_message| {
                            error(
                                item,
                                instruction.span,
                                format!("const.int encoding failed: {error_message}"),
                            )
                        },
                    )?;
                }
                ParsedEntryConstant::Boolean(value) => {
                    write(0x002, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    crate::leb::write_integer_immediate(false, u128::from(value.value), &mut bytes)
                        .map_err(|error_message| {
                            error(
                                item,
                                instruction.span,
                                format!("const.bool encoding failed: {error_message}"),
                            )
                        })?;
                }
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "unsupported constant in a straight-line function",
                    ));
                }
            }
        } else if crate::text_memory_assembly::handles(&instruction.opcode) {
            let parsed = crate::text_memory_assembly::parse(instruction, source, item)?;
            parsed.encode(values, types, &mut bytes, item, instruction.span)?;
        } else if matches!(
            instruction.opcode.as_str(),
            "copy" | "move" | "destroy" | "borrow.begin" | "borrow.end"
        ) {
            let parsed = crate::text_ownership_assembly::parse(instruction, item)?;
            let position = values
                .iter()
                .position(|name| name == &parsed.operand)
                .ok_or_else(|| error(item, instruction.span, "ownership operand is undefined"))?;
            write(parsed.opcode, &mut bytes, item, instruction.span)?;
            write(0, &mut bytes, item, instruction.span)?;
            write(
                u64::from(parsed.result.is_some()),
                &mut bytes,
                item,
                instruction.span,
            )?;
            write(
                (values.len() - position) as u64,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if instruction.opcode == "select" {
            let parsed = parse_select(instruction, item)?;
            write(0x03d, &mut bytes, item, instruction.span)?;
            write(0, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            for operand in [&parsed.condition, &parsed.when_true, &parsed.when_false] {
                let position = values
                    .iter()
                    .position(|name| name == operand)
                    .ok_or_else(|| error(item, instruction.span, "undefined SSA operand"))?;
                write(
                    (values.len() - position) as u64,
                    &mut bytes,
                    item,
                    instruction.span,
                )?;
            }
        } else {
            return Err(error(
                item,
                instruction.span,
                "unsupported straight-line instruction",
            ));
        }
        if let Some(result) = instruction.results.first() {
            values.push(result);
        }
    }
    Ok(())
}

fn write(
    value: u64,
    output: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    crate::leb::write_uleb(value, output)
        .map_err(|err| error(item, span, format!("CODE encoding failed: {err}")))
}

fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    fn module_with_body(body: &str) -> String {
        format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.chain(%a: i32, %b: i32) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32):\n{body}\n}}\n"
        )
    }

    #[test]
    fn integer_chain_is_canonical_after_disassembly() {
        let source = module_with_body(
            "  %sum = add %a, %b\n  %product = mul %sum, %b\n  %result = sub %product, %a\n  return %result",
        );
        let text = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&text).expect("chain assembles");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let disassembled = disassemble_text_module(&module, u64::MAX).expect("chain disassembles");
        let round_trip =
            assemble_text_module(&parse_text_module(&disassembled).expect("text reparses"))
                .expect("binary reassembles");
        assert_eq!(bytes, round_trip);
    }

    #[test]
    fn intermediate_icmp_i1_type_is_interned_and_round_trips() {
        let source = module_with_body(
            "  %equal = icmp eq %a, %b\n  %sum = add %a, %b\n  %result = select %equal, %sum, %a\n  return %result",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("icmp chain assembles");
        let module = BinaryModule::decode(&bytes).expect("binary decodes");
        let types = module.type_table(u64::MAX).expect("type table decodes");
        assert!(
            types
                .entries
                .iter()
                .any(|ty| matches!(ty, crate::BinaryType::Integer(1)))
        );
        let text = disassemble_text_module(&module, u64::MAX).expect("icmp chain disassembles");
        let reassembled = assemble_text_module(&parse_text_module(&text).expect("text parses"))
            .expect("round trip assembles");
        assert_eq!(bytes, reassembled);
    }

    #[test]
    fn mixed_constants_arithmetic_and_compare_round_trip() {
        let source = module_with_body(
            "  %five = const.int i32, 5\n  %sum = add %a, %five\n  %same = icmp eq %sum, %b\n  %flag = const.bool true\n  %keep = and %same, %flag\n  %value = select %keep, %sum, %b\n  return %value",
        );
        let parsed = parse_text_module(&source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("mixed function assembles");
        let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&decoded, u64::MAX).expect("binary disassembles");
        let reassembled = assemble_text_module(&parse_text_module(&text).expect("text parses"))
            .expect("binary reassembles");
        assert_eq!(bytes, reassembled);
    }

    #[test]
    fn undefined_or_mismatched_chain_inputs_are_rejected() {
        for body in [
            "  %sum = add %a, %missing\n  %product = mul %sum, %b\n  return %product",
            "  %sum = icmp eq %a, %b\n  %product = add %sum, %a\n  return %product",
            "  %sum = add %a, %b\n  %sum = mul %sum, %b\n  return %sum",
        ] {
            let text = parse_text_module(&module_with_body(body)).expect("text parses");
            assert!(
                assemble_text_module(&text).is_err(),
                "invalid body assembled: {body}"
            );
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    #[test]
    fn explicit_copy_move_destroy_round_trip() {
        let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.own(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %dup = copy %a\n  %taken = move %dup\n  destroy %a\n  %sum = add %taken, %b\n  return %sum\n}\n";
        let parsed = parse_text_module(source).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("ownership ops assemble");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let text = disassemble_text_module(&binary, u64::MAX).expect("ownership ops disassemble");
        let rebuilt =
            assemble_text_module(&parse_text_module(&text).expect("canonical text reparses"))
                .expect("canonical text reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn ownership_instruction_arity_is_enforced() {
        let body = "  %dup = copy %a, %b\n  destroy %dup\n  %sum = add %a, %b\n  return %sum";
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.own(%a: i32, %b: i32) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32):\n{body}\n}}\n"
        );
        let parsed = parse_text_module(&source).expect("text parses");
        assert!(assemble_text_module(&parsed).is_err());
    }
}
