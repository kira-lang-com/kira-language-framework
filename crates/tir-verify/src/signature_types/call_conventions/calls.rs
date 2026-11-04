use super::{
    CallConventions, argument_names_at, is_punctuation, vtable_call_operands, witness_call_operands,
};
use std::collections::HashMap;
use tir_types::{Convention, Type};

impl CallConventions {
    pub(crate) fn get(&self, symbol: &str) -> Option<&Vec<Option<Convention>>> {
        self.direct.get(symbol)
    }

    pub(crate) fn direct_generic_parameter_count(&self, symbol: &str) -> Option<usize> {
        self.direct_parameter_types.get(symbol)?;
        Some(
            self.direct_generic_parameters
                .get(symbol)
                .map_or(0, Vec::len),
        )
    }

    pub(crate) fn explicit_type_argument_count(
        &self,
        instruction: &tir_format::TextInstruction,
    ) -> Option<usize> {
        super::generic_calls::explicit_type_argument_count(instruction)
    }

    pub(crate) fn direct_call_parameter_types(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
        symbol: &str,
        value_types: &HashMap<String, Type>,
    ) -> Option<Vec<Type>> {
        let mut parameters = self.direct_parameter_types.get(symbol)?.clone();
        let Some(generic_parameters) = self.direct_generic_parameters.get(symbol) else {
            return Some(parameters);
        };
        let bindings = if super::generic_calls::explicit_type_argument_count(instruction).is_some()
        {
            super::generic_calls::call_bindings(source, instruction, generic_parameters)?
        } else if matches!(instruction.opcode.as_str(), "try.call" | "invoke") {
            super::generic_calls::infer_call_bindings(
                instruction,
                &parameters,
                generic_parameters,
                value_types,
            )?
        } else {
            return Some(parameters);
        };
        for parameter in &mut parameters {
            *parameter = parameter.substitute(&bindings);
        }
        Some(parameters)
    }

    pub(crate) fn direct_call_edge_shape(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
        symbol: &str,
        value_types: &HashMap<String, Type>,
    ) -> Option<(usize, Option<Type>, Option<Type>)> {
        let signature = self.direct_signatures.get(symbol)?;
        let bindings = if let Some(generic_parameters) = self.direct_generic_parameters.get(symbol)
        {
            if super::generic_calls::explicit_type_argument_count(instruction).is_some() {
                super::generic_calls::call_bindings(source, instruction, generic_parameters)?
            } else {
                super::generic_calls::infer_call_bindings(
                    instruction,
                    self.direct_parameter_types.get(symbol)?,
                    generic_parameters,
                    value_types,
                )?
            }
        } else {
            HashMap::new()
        };
        let result = signature.result.substitute(&bindings);
        let (result_count, result_type) =
            super::call_result_shape(result, signature.asynchronous, Some(symbol));
        let failure = signature
            .failure
            .as_deref()
            .map(|failure| failure.substitute(&bindings));
        Some((result_count, result_type, failure))
    }

    pub(crate) fn call_result_shape(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
        value_types: &HashMap<String, Type>,
    ) -> Option<(usize, Option<Type>)> {
        match instruction.opcode.as_str() {
            "call.indirect" => {
                let (function, _) = super::super::indirect_call_type(source, instruction)?;
                Some(super::call_result_shape(
                    *function.result,
                    function.asynchronous,
                    None,
                ))
            }
            "call.closure" => {
                let uses = crate::ssa::text_value_uses(instruction);
                let (name, _) = uses.first()?;
                let Type::Closure(closure) = value_types.get(name)? else {
                    return None;
                };
                Some(super::call_result_shape(
                    *closure.signature.result.clone(),
                    closure.signature.asynchronous,
                    None,
                ))
            }
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
            "call" | "call.foreign" | "call.variadic" => {
                let (symbol, _, _) =
                    instruction
                        .tokens
                        .iter()
                        .enumerate()
                        .find_map(|(index, token)| {
                            is_punctuation(token, '@')
                                .then(|| {
                                    crate::variant_text::read_symbol(&instruction.tokens, index)
                                })
                                .flatten()
                        })?;
                let mut result = self.direct_results.get(&symbol)?.clone();
                if let Some(generic_parameters) = self.direct_generic_parameters.get(&symbol)
                    && let Some(bindings) =
                        super::generic_calls::call_bindings(source, instruction, generic_parameters)
                    && let Some(ty) = result.1.take()
                {
                    result = super::call_result_shape(ty.substitute(&bindings), false, None);
                }
                Some(result)
            }
            _ => None,
        }
    }

    pub(crate) fn inferred_result_types(
        &self,
        instruction: &tir_format::TextInstruction,
        source: &str,
        value_types: &HashMap<String, Type>,
    ) -> Option<Vec<Type>> {
        let (count, ty) = self.call_result_shape(instruction, source, value_types)?;
        if count == 0 {
            return Some(Vec::new());
        }
        let ty = ty?;
        ty.canonical_text_in(&self.module_path, &HashMap::new())
            .ok()?;
        Some(vec![ty])
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
