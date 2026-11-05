use std::collections::HashMap;

use super::{
    CallConventions, PlatformImport, argument_names_at, vtable_call_operands, witness_call_operands,
};
use tir_format::parse_type;
use tir_format::text::TokenKind;
use tir_types::{Abi, AddressSpace, Convention, Type};

impl CallConventions {
    pub(crate) fn specialized_implicit_call(
        &self,
        _source: &str,
        instruction: &tir_format::TextInstruction,
        value_types: &HashMap<String, Type>,
    ) -> Option<Result<tir_types::FunctionType, String>> {
        if !matches!(instruction.opcode.as_str(), "try.call" | "invoke") {
            return None;
        }
        let (symbol, symbol_end) = instruction_symbol_parts(instruction)?;
        let function = self.function_references.get(&symbol)?;
        let explicit = instruction
            .tokens
            .get(symbol_end)
            .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('<')));
        let Some(parameters) = self.generic_function_parameters.get(&symbol) else {
            return Some(if explicit {
                Err(format!("non-generic function @{symbol} has type arguments"))
            } else {
                Ok(function.clone())
            });
        };
        if explicit {
            return Some(Err(format!(
                "{} infers generic type arguments from its value arguments",
                instruction.opcode
            )));
        }
        let Some(parameter_types) = self.direct_parameter_types.get(&symbol) else {
            return Some(Err(format!(
                "cannot resolve generic type arguments for {} from its call",
                instruction.opcode
            )));
        };
        let Some(substitutions) = super::generic_calls::infer_call_bindings(
            instruction,
            parameter_types,
            parameters,
            value_types,
        ) else {
            return Some(Err(format!(
                "cannot resolve generic type arguments for {} from its call",
                instruction.opcode
            )));
        };

        let tir_types::Type::Function(function) =
            tir_types::Type::Function(function.clone()).substitute(&substitutions)
        else {
            unreachable!("substituting a function type preserves its outer type")
        };
        Some(Ok(function))
    }

    pub(crate) fn specialized_direct_call(
        &self,
        source: &str,
        instruction: &tir_format::TextInstruction,
    ) -> Option<Result<tir_types::FunctionType, String>> {
        if !matches!(instruction.opcode.as_str(), "call" | "tail.call") {
            return None;
        }
        let (symbol, symbol_end) = instruction_symbol_parts(instruction)?;
        let function = self.function_references.get(&symbol);
        if function.is_none() && !self.direct_parameter_types.contains_key(&symbol) {
            return None;
        }
        let type_arguments =
            match explicit_type_arguments(source, instruction, symbol_end, "direct call") {
                Ok(arguments) => arguments,
                Err(message) => return Some(Err(message)),
            };
        let Some(function) = function else {
            return if type_arguments.is_empty() {
                None
            } else {
                Some(Err(format!(
                    "non-generic function @{symbol} has type arguments"
                )))
            };
        };
        let Some(parameters) = self.generic_function_parameters.get(&symbol) else {
            return if type_arguments.is_empty() {
                Some(Ok(function.clone()))
            } else {
                Some(Err(format!(
                    "non-generic function @{symbol} has type arguments"
                )))
            };
        };
        let argument_count =
            super::generic_calls::explicit_type_argument_count(instruction).unwrap_or_default();
        if argument_count != parameters.len() {
            return Some(Err(format!(
                "call to @{symbol} has {} type arguments; expected {}",
                argument_count,
                parameters.len()
            )));
        }
        let Some(substitutions) =
            super::generic_calls::call_bindings(source, instruction, parameters)
        else {
            return Some(Err(format!(
                "call to @{symbol} has invalid explicit type arguments"
            )));
        };
        let tir_types::Type::Function(function) =
            tir_types::Type::Function(function.clone()).substitute(&substitutions)
        else {
            unreachable!("substituting a function type preserves its outer type")
        };
        Some(Ok(function))
    }

    pub(crate) fn function_reference_result_type(
        &self,
        source: &str,
        instruction: &tir_format::TextInstruction,
    ) -> Result<Type, String> {
        let (symbol, symbol_end) = instruction_symbol_parts(instruction)
            .ok_or_else(|| "func.ref must name a function".to_owned())?;
        let Some(function) = self.function_references.get(&symbol) else {
            return Err(format!("func.ref names unknown function @{symbol}"));
        };
        let arguments = explicit_type_arguments(source, instruction, symbol_end, "func.ref")?;
        let Some(parameters) = self.generic_function_parameters.get(&symbol) else {
            if arguments.is_empty() {
                return Ok(Type::Function(function.clone()));
            }
            return Err(format!("non-generic function @{symbol} has type arguments"));
        };
        if arguments.len() != parameters.len() {
            return Err(format!(
                "func.ref @{symbol} has {} type arguments; expected {}",
                arguments.len(),
                parameters.len()
            ));
        }
        let substitutions = parameters.iter().cloned().zip(arguments).collect();
        Ok(Type::Function(function.clone()).substitute(&substitutions))
    }

    pub(crate) fn verify_foreign_call_target(
        &self,
        instruction: &tir_format::TextInstruction,
    ) -> Result<(), String> {
        let symbol = instruction_symbol(instruction)
            .ok_or_else(|| "call.foreign must name an imported function".to_owned())?;
        if !self.imported_functions.contains(&symbol) {
            return Err("call.foreign must name an imported function".to_owned());
        }
        if self.direct_c_abi.get(&symbol) != Some(&true) {
            return Err("call.foreign requires an imported function with abi(c)".to_owned());
        }
        Ok(())
    }

    pub(crate) fn symbol_address_result_type(
        &self,
        instruction: &tir_format::TextInstruction,
    ) -> Result<Type, String> {
        let symbol = instruction_symbol(instruction)
            .ok_or_else(|| format!("{} must name a symbol", instruction.opcode))?;

        match instruction.opcode.as_str() {
            "kernel.percpu.addr" => {
                if self.per_cpu_globals.contains(&symbol) {
                    Ok(Type::Pointer(AddressSpace::Generic))
                } else {
                    Err("kernel.percpu.addr must name a defined per-CPU global".to_owned())
                }
            }
            "foreign.addr" => match self.platform_imports.get(&symbol) {
                Some(PlatformImport::Function(function)) if function.abi == Abi::C => {
                    let mut function = function.clone();
                    function.foreign = true;
                    Ok(Type::Function(function))
                }
                Some(PlatformImport::Function(_)) => {
                    Err("foreign.addr requires an imported function with abi(c)".to_owned())
                }
                _ => Err("foreign.addr must name an imported platform function".to_owned()),
            },
            "foreign.data" => match self.platform_imports.get(&symbol) {
                Some(PlatformImport::Data) => Ok(Type::Pointer(AddressSpace::Generic)),
                _ => Err("foreign.data must name an imported platform data object".to_owned()),
            },
            "foreign.entry" => match self.defined_functions.get(&symbol) {
                Some(function) if function.abi == Abi::C => {
                    let mut function = function.clone();
                    function.foreign = true;
                    Ok(Type::Function(function))
                }
                _ => Err("foreign.entry must name a defined C-ABI function".to_owned()),
            },
            _ => Err(format!(
                "{} is not a supported symbol-address instruction",
                instruction.opcode
            )),
        }
    }

    pub(crate) fn get(&self, symbol: &str) -> Option<&Vec<Option<Convention>>> {
        self.direct.get(symbol)
    }

    pub(crate) fn direct_parameter_types(&self, symbol: &str) -> Option<Vec<Type>> {
        self.direct_parameter_types.get(symbol).cloned()
    }

    pub(crate) fn direct_result(&self, symbol: &str) -> Option<super::DirectResult> {
        self.direct_results.get(symbol).cloned()
    }

    pub(crate) fn special_parameter_types(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
    ) -> Option<(Vec<String>, Vec<Type>)> {
        match instruction.opcode.as_str() {
            "call.witness" => {
                let (ty, trait_path, method, open) =
                    witness_call_operands(instruction, source, &self.module_path)?;
                let parameter_types = ty
                    .as_ref()
                    .and_then(|ty| {
                        self.witness_parameter_types.get(&(
                            ty.clone(),
                            trait_path.clone(),
                            method.clone(),
                        ))
                    })
                    .or_else(|| self.trait_parameter_types.get(&(trait_path, method)))?
                    .clone();
                Some((
                    argument_names_at(&instruction.tokens, open),
                    parameter_types,
                ))
            }
            "call.vtable" => {
                let (symbol, slot, receiver, open) = vtable_call_operands(instruction)?;
                let parameter_types = self.vtable_parameter_types.get(&(symbol, slot))?.clone();
                let mut arguments = vec![receiver];
                arguments.extend(argument_names_at(&instruction.tokens, open));
                Some((arguments, parameter_types))
            }
            _ => None,
        }
    }

    pub(crate) fn special_result(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
    ) -> Option<super::DirectResult> {
        match instruction.opcode.as_str() {
            "call.witness" => {
                let (ty, trait_path, method, _) =
                    witness_call_operands(instruction, source, &self.module_path)?;
                ty.as_ref()
                    .and_then(|ty| {
                        self.witness_results
                            .get(&(ty.clone(), trait_path.clone(), method.clone()))
                    })
                    .or_else(|| self.trait_results.get(&(trait_path, method)))
                    .cloned()
            }
            "call.vtable" => {
                let (symbol, slot, _, _) = vtable_call_operands(instruction)?;
                self.vtable_results.get(&(symbol, slot)).cloned()
            }
            _ => None,
        }
    }

    pub(crate) fn module_path(&self) -> &str {
        &self.module_path
    }

    pub(crate) fn special_call(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
    ) -> Option<(Vec<String>, Vec<Option<Convention>>)> {
        match instruction.opcode.as_str() {
            "call.witness" => {
                let (ty, trait_path, method, open) =
                    witness_call_operands(instruction, source, &self.module_path)?;
                let conventions = ty
                    .as_ref()
                    .and_then(|ty| {
                        self.witnesses
                            .get(&(ty.clone(), trait_path.clone(), method.clone()))
                    })
                    .or_else(|| self.trait_methods.get(&(trait_path, method)))?
                    .clone();
                Some((argument_names_at(&instruction.tokens, open), conventions))
            }
            "call.vtable" => {
                let (symbol, slot, receiver, open) = vtable_call_operands(instruction)?;
                let conventions = self.vtables.get(&(symbol, slot))?.clone();
                let mut arguments = vec![receiver];
                arguments.extend(argument_names_at(&instruction.tokens, open));
                Some((arguments, conventions))
            }
            _ => None,
        }
    }

    pub(crate) fn special_call_failure(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
    ) -> Option<bool> {
        match instruction.opcode.as_str() {
            "call.witness" => {
                let (ty, trait_path, method, _) =
                    witness_call_operands(instruction, source, &self.module_path)?;
                ty.as_ref()
                    .and_then(|ty| {
                        self.witness_failures
                            .get(&(ty.clone(), trait_path.clone(), method.clone()))
                    })
                    .or_else(|| self.trait_failures.get(&(trait_path, method)))
                    .copied()
            }
            "call.vtable" => {
                let (symbol, slot, _, _) = vtable_call_operands(instruction)?;
                self.vtable_failures.get(&(symbol, slot)).copied()
            }
            _ => None,
        }
    }
}

fn instruction_symbol(instruction: &tir_format::TextInstruction) -> Option<String> {
    instruction_symbol_parts(instruction).map(|(symbol, _)| symbol)
}

fn instruction_symbol_parts(instruction: &tir_format::TextInstruction) -> Option<(String, usize)> {
    let start = crate::ssa::text_instruction_operands_start(&instruction.tokens);
    instruction
        .tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                .flatten()
                .map(|(symbol, symbol_end, _)| (symbol, symbol_end))
        })
}

fn explicit_type_arguments(
    source: &str,
    instruction: &tir_format::TextInstruction,
    symbol_end: usize,
    subject: &str,
) -> Result<Vec<Type>, String> {
    let tokens = &instruction.tokens;
    if !tokens
        .get(symbol_end)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('<')))
    {
        return Ok(Vec::new());
    }
    let mut angle_depth = 0i32;
    let mut close = None;
    for (index, token) in tokens.iter().enumerate().skip(symbol_end) {
        match token.kind {
            TokenKind::Punctuation('<') => angle_depth += 1,
            TokenKind::Punctuation('>') => {
                angle_depth -= 1;
                if angle_depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Err(format!("{subject} has unterminated type arguments"));
    };
    let mut ranges = Vec::new();
    let mut start = symbol_end + 1;
    let mut nesting = [0i32; 4];
    for index in symbol_end + 1..close {
        match tokens[index].kind {
            TokenKind::Punctuation('(') => nesting[0] += 1,
            TokenKind::Punctuation(')') => nesting[0] -= 1,
            TokenKind::Punctuation('[') => nesting[1] += 1,
            TokenKind::Punctuation(']') => nesting[1] -= 1,
            TokenKind::Punctuation('<') => nesting[2] += 1,
            TokenKind::Punctuation('>') => nesting[2] -= 1,
            TokenKind::Punctuation('{') => nesting[3] += 1,
            TokenKind::Punctuation('}') => nesting[3] -= 1,
            TokenKind::Punctuation(',') if nesting == [0; 4] => {
                ranges.push((start, index));
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < close {
        ranges.push((start, close));
    } else if !ranges.is_empty() {
        return Err("func.ref has an empty type argument".to_owned());
    }
    ranges
        .into_iter()
        .map(|(start, end)| {
            let first = tokens
                .get(start)
                .ok_or_else(|| format!("{subject} has an empty type argument"))?;
            let last = tokens
                .get(end.saturating_sub(1))
                .ok_or_else(|| format!("{subject} has an empty type argument"))?;
            let text = source
                .get(first.span.start..last.span.end)
                .ok_or_else(|| format!("{subject} has an invalid type argument span"))?;
            parse_type(text).map_err(|error| error.to_string())
        })
        .collect()
}
