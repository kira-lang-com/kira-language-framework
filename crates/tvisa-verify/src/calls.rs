use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::FunctionBody;
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind, lex};
use tir_types::Type;
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind, TextSignature};

use crate::call_types::{
    infer_instruction_results, signature_parameter_types, signature_result_types,
    verify_indirect_callee, verify_runtime_call,
};
use crate::symbols::{instruction_symbol_operand, symbol_references};

const VFY5: Code = Code::new(Dialect::Tvisa, Area::Verifier, 5);
const VFY30: Code = Code::new(Dialect::Tvisa, Area::Verifier, 30);

pub(super) struct SignatureIndex {
    functions: HashMap<String, String>,
    sets: HashMap<String, Vec<String>>,
}

impl SignatureIndex {
    pub(super) fn new(image: &TextImage) -> Self {
        let (functions, sets) = signature_sources(image);
        Self { functions, sets }
    }

    pub(super) fn verify_function(
        &self,
        image: &TextImage,
        caller: &TextSignature,
        source: &str,
        body: &FunctionBody,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut value_types = HashMap::new();
        let slot_types = crate::slots::pointer_types(source, caller, body);
        let gpu_function = matches!(
            caller.convention,
            TextCallingConvention::Gpu | TextCallingConvention::Entry
        );
        for block in &body.blocks {
            for parameter in &block.parameters {
                diagnostics.extend(crate::signature::verify_body_value_type(
                    &parameter.ty,
                    gpu_function,
                    parameter.span,
                ));
                value_types.insert(parameter.name.clone(), parameter.ty.clone());
            }
        }

        for block in &body.blocks {
            for instruction in &block.instructions {
                diagnostics.extend(crate::instruction_types::verify_instruction(
                    source,
                    instruction,
                    &value_types,
                    gpu_function,
                ));
                if instruction.opcode == "call.runtime" {
                    if let Some((runtime_diagnostics, results)) =
                        verify_runtime_call(instruction, &value_types)
                    {
                        diagnostics.extend(runtime_diagnostics);
                        value_types.extend(results);
                    }
                    continue;
                }
                let signature = self.call_signature(image, instruction, &mut diagnostics);
                if let Some(callee) = signature {
                    diagnostics.extend(self.verify_call(
                        image,
                        instruction,
                        callee,
                        &value_types,
                        body,
                    ));
                    if instruction.opcode == "tail.call" {
                        diagnostics.extend(verify_ridl_tail_call(caller, callee, instruction));
                    } else if instruction.opcode != "invoke" {
                        for (name, ty) in instruction
                            .results
                            .iter()
                            .zip(signature_result_types(callee))
                        {
                            value_types.insert(name.clone(), ty);
                        }
                    }
                }
                for (name, ty) in infer_instruction_results(
                    source,
                    instruction,
                    &value_types,
                    &slot_types,
                    gpu_function,
                ) {
                    diagnostics.extend(crate::signature::verify_body_value_type(
                        &ty,
                        gpu_function,
                        instruction.span,
                    ));
                    value_types.insert(name, ty);
                }
            }
        }
        diagnostics.extend(crate::edge_types::verify_function(body, &value_types));
        diagnostics
    }

    fn call_signature<'a>(
        &self,
        image: &'a TextImage,
        instruction: &TextInstruction,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<&'a TextSignature> {
        if !is_call(instruction) {
            return None;
        }
        let explicit = matches!(
            instruction.opcode.as_str(),
            "call.indirect" | "call.variadic"
        ) || (matches!(instruction.opcode.as_str(), "tail.call" | "invoke")
            && signature_reference(&instruction.tokens).is_some());
        let name = if explicit {
            let Some(name) = signature_reference(&instruction.tokens) else {
                diagnostics.push(
                    Diagnostic::error(VFY5, "call form requires an explicit signature reference")
                        .with_primary_span(instruction.span),
                );
                return None;
            };
            if instruction.opcode == "call.variadic"
                && let Some((symbol, _)) = instruction_symbol_operand(instruction)
                && let Some(target_signature) =
                    resolve_signature_name(&symbol, &self.functions, &self.sets)
                && target_signature != name
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY5,
                        format!(
                            "call.variadic signature ${name} does not match function @{symbol} signature ${target_signature}"
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
            name
        } else {
            let Some((symbol, _)) = instruction_symbol_operand(instruction) else {
                return None;
            };
            let Some(name) = resolve_signature_name(&symbol, &self.functions, &self.sets) else {
                return None;
            };
            name
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|candidate| candidate.name == name)
        else {
            diagnostics.push(
                Diagnostic::error(VFY5, format!("call refers to unknown signature ${name}"))
                    .with_primary_span(instruction.span),
            );
            return None;
        };
        if instruction.opcode == "call.variadic" {
            verify_variadic_form(instruction, signature, diagnostics);
        } else if signature.variadic.is_some() {
            diagnostics.push(
                Diagnostic::error(VFY5, "calls to variadic signatures must use call.variadic")
                    .with_primary_span(instruction.span),
            );
        }
        Some(signature)
    }

    fn verify_call(
        &self,
        _image: &TextImage,
        instruction: &TextInstruction,
        signature: &TextSignature,
        value_types: &HashMap<String, Type>,
        body: &FunctionBody,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let is_indirect = instruction.opcode == "call.indirect"
            || (matches!(instruction.opcode.as_str(), "tail.call" | "invoke")
                && signature_reference(&instruction.tokens).is_some());
        diagnostics.extend(verify_indirect_callee(
            instruction,
            value_types,
            is_indirect,
        ));
        let Some(arguments) = call_arguments(instruction) else {
            diagnostics.push(
                Diagnostic::error(VFY5, "call is missing its argument list")
                    .with_primary_span(instruction.span),
            );
            return diagnostics;
        };
        if !arguments.valid {
            diagnostics.push(
                Diagnostic::error(
                    VFY5,
                    "call arguments must be SSA values separated by commas",
                )
                .with_primary_span(instruction.span),
            );
        }
        let variadic_fixed = if instruction.opcode == "call.variadic" {
            fixed_parameter_count(instruction)
        } else {
            None
        };
        let parameter_count = variadic_fixed
            .unwrap_or(signature.parameters.len())
            .min(signature.parameters.len());
        let expected = signature_parameter_types(signature, parameter_count);
        if instruction.opcode == "call.variadic" {
            if arguments.values.len() < expected.len() {
                diagnostics.push(
                    Diagnostic::error(
                        VFY5,
                        format!(
                            "call.variadic has {} arguments for {} fixed parameter leaves",
                            arguments.values.len(),
                            expected.len()
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
        } else if arguments.values.len() != expected.len() {
            diagnostics.push(
                Diagnostic::error(
                    VFY5,
                    format!(
                        "call has {} arguments; callee signature requires {} leaves",
                        arguments.values.len(),
                        expected.len()
                    ),
                )
                .with_primary_span(instruction.span),
            );
        }
        for (argument, expected_type) in arguments.values.iter().zip(&expected) {
            if let Some(actual_type) = value_types.get(&argument.name)
                && actual_type != expected_type
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY5,
                        format!(
                            "call argument %{} has type {actual_type}, expected {expected_type}",
                            argument.name
                        ),
                    )
                    .with_primary_span(argument.span),
                );
            }
        }

        if instruction.opcode == "tail.call" {
            if !instruction.results.is_empty() {
                diagnostics.push(
                    Diagnostic::error(VFY5, "tail.call cannot define result values")
                        .with_primary_span(instruction.span),
                );
            }
        } else if instruction.opcode == "invoke" {
            verify_invoke_results(
                instruction,
                signature,
                &arguments,
                value_types,
                body,
                &mut diagnostics,
            );
        } else {
            let expected_results = signature_result_types(signature);
            if instruction.results.len() != expected_results.len() {
                diagnostics.push(
                    Diagnostic::error(
                        VFY5,
                        format!(
                            "call defines {} results; callee signature yields {} leaves",
                            instruction.results.len(),
                            expected_results.len()
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
        }
        diagnostics
    }
}

#[derive(Clone, Debug)]
struct ValueOperand {
    name: String,
    span: Span,
}

struct ArgumentList {
    values: Vec<ValueOperand>,
    close: usize,
    valid: bool,
}

fn verify_ridl_tail_call(
    caller: &TextSignature,
    callee: &TextSignature,
    instruction: &TextInstruction,
) -> Vec<Diagnostic> {
    if callee.convention != TextCallingConvention::Ridl {
        return Vec::new();
    }
    if caller.convention == TextCallingConvention::Ridl
        && caller.fails == callee.fails
        && caller.results == callee.results
    {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                VFY30,
                "tail.call to a ridl function requires a ridl caller with identical results and failing status",
            )
            .with_primary_span(instruction.span),
        ]
    }
}

fn verify_variadic_form(
    instruction: &TextInstruction,
    signature: &TextSignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if signature.convention != TextCallingConvention::C || signature.variadic.is_none() {
        diagnostics.push(
            Diagnostic::error(VFY5, "call.variadic requires a variadic c signature")
                .with_primary_span(instruction.span),
        );
        return;
    }
    let Some(fixed) = fixed_parameter_count(instruction) else {
        diagnostics.push(
            Diagnostic::error(VFY5, "call.variadic requires a fixed parameter count")
                .with_primary_span(instruction.span),
        );
        return;
    };
    if u32::try_from(fixed).ok() != signature.variadic {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                format!(
                    "call.variadic fixed count {fixed} does not match signature count {}",
                    signature.variadic.unwrap_or_default()
                ),
            )
            .with_primary_span(instruction.span),
        );
    }
}

fn verify_invoke_results(
    instruction: &TextInstruction,
    signature: &TextSignature,
    call_arguments: &ArgumentList,
    value_types: &HashMap<String, Type>,
    body: &FunctionBody,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !instruction.results.is_empty() {
        diagnostics.push(
            Diagnostic::error(VFY5, "invoke passes results through its normal successor")
                .with_primary_span(instruction.span),
        );
    }
    let Some((target, explicit)) = invoke_normal_arguments(instruction, call_arguments) else {
        diagnostics.push(
            Diagnostic::error(VFY5, "invoke is missing its normal successor")
                .with_primary_span(instruction.span),
        );
        return;
    };
    if !explicit.valid {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                "invoke successor arguments must be SSA values separated by commas",
            )
            .with_primary_span(instruction.span),
        );
    }
    let Some(block) = body.blocks.iter().find(|block| block.label == target) else {
        return;
    };
    let results = signature_result_types(signature);
    if block.parameters.len() != results.len() + explicit.values.len() {
        diagnostics.push(
            Diagnostic::error(
                VFY5,
                format!(
                    "invoke successor {} has {} parameters; expected {} call results and {} explicit arguments",
                    block.label,
                    block.parameters.len(),
                    results.len(),
                    explicit.values.len()
                ),
            )
            .with_primary_span(instruction.span),
        );
    }
    for (parameter, result_type) in block.parameters.iter().zip(&results) {
        if &parameter.ty != result_type {
            diagnostics.push(
                Diagnostic::error(
                    VFY5,
                    format!(
                        "invoke result for %{} has type {}; successor parameter has type {}",
                        parameter.name, result_type, parameter.ty
                    ),
                )
                .with_primary_span(parameter.span),
            );
        }
    }
    for (parameter, argument) in block
        .parameters
        .iter()
        .skip(results.len())
        .zip(&explicit.values)
    {
        if let Some(actual_type) = value_types.get(&argument.name)
            && actual_type != &parameter.ty
        {
            diagnostics.push(
                Diagnostic::error(
                    VFY5,
                    format!(
                        "invoke successor argument %{} has type {actual_type}, expected {}",
                        argument.name, parameter.ty
                    ),
                )
                .with_primary_span(argument.span),
            );
        }
    }
}

fn invoke_normal_arguments(
    instruction: &TextInstruction,
    call_arguments: &ArgumentList,
) -> Option<(String, ArgumentList)> {
    let tokens = &instruction.tokens;
    let to = tokens
        .iter()
        .enumerate()
        .skip(call_arguments.close + 1)
        .find(|(_, token)| is_atom(token, "to"))
        .map(|(index, _)| index)?;
    let target = atom(tokens.get(to + 1)?)?.to_owned();
    let open = (to + 2..tokens.len()).find(|index| {
        tokens
            .get(*index)
            .is_some_and(|token| is_punctuation(token, '('))
    })?;
    let close = matching_paren(tokens, open)?;
    Some((target, parse_value_list(tokens, open, close)))
}

fn call_arguments(instruction: &TextInstruction) -> Option<ArgumentList> {
    let start = opcode_end(instruction);
    let open = (start..instruction.tokens.len()).find(|index| {
        instruction
            .tokens
            .get(*index)
            .is_some_and(|token| is_punctuation(token, '('))
    })?;
    let close = matching_paren(&instruction.tokens, open)?;
    Some(parse_value_list(&instruction.tokens, open, close))
}

fn parse_value_list(tokens: &[Token], open: usize, close: usize) -> ArgumentList {
    let mut values = Vec::new();
    let mut start = open + 1;
    let mut valid = true;
    for index in open + 1..close {
        if is_punctuation(&tokens[index], ',') {
            valid &= push_value(&mut values, &tokens[start..index]);
            start = index + 1;
        }
    }
    if start < close {
        valid &= push_value(&mut values, &tokens[start..close]);
    } else if start != open + 1 {
        valid = false;
    }
    ArgumentList {
        values,
        close,
        valid,
    }
}

fn push_value(values: &mut Vec<ValueOperand>, tokens: &[Token]) -> bool {
    if let Some(value) = parse_value(tokens) {
        values.push(value);
        true
    } else {
        false
    }
}

fn parse_value(tokens: &[Token]) -> Option<ValueOperand> {
    if tokens.len() != 2 || !is_punctuation(&tokens[0], '%') {
        return None;
    }
    let name_token = &tokens[1];
    Some(ValueOperand {
        name: atom(name_token)?.to_owned(),
        span: Span::new(tokens[0].span.start, name_token.span.end),
    })
}

fn fixed_parameter_count(instruction: &TextInstruction) -> Option<usize> {
    let tokens = &instruction.tokens;
    let index = tokens.iter().position(|token| is_atom(token, "fixed"))?;
    number(tokens.get(index + 1)?)
}

fn signature_reference(tokens: &[Token]) -> Option<String> {
    tokens.windows(2).find_map(|window| {
        is_punctuation(&window[0], '$')
            .then(|| atom(&window[1]).map(str::to_owned))
            .flatten()
    })
}

fn is_call(instruction: &TextInstruction) -> bool {
    matches!(
        instruction.opcode.as_str(),
        "call" | "call.indirect" | "call.runtime" | "call.variadic" | "tail.call" | "invoke"
    )
}

fn signature_sources(image: &TextImage) -> (HashMap<String, String>, HashMap<String, Vec<String>>) {
    let mut functions = HashMap::new();
    let mut sets = HashMap::new();
    for item in &image.items {
        if !matches!(
            item.kind,
            TextItemKind::Function
                | TextItemKind::Import
                | TextItemKind::Variants
                | TextItemKind::Dispatch
        ) {
            continue;
        }
        let Ok(tokens) = lex(image.item_text(*item)) else {
            continue;
        };
        let tokens = significant(&tokens);
        let references = symbol_references(&tokens);
        let Some(symbol) = references.first().cloned() else {
            continue;
        };
        match item.kind {
            TextItemKind::Function => {
                if let Some(signature) = signature_reference_refs(&tokens) {
                    functions.insert(symbol, signature);
                }
            }
            TextItemKind::Import => {
                if tokens.first().is_some_and(|token| is_atom(token, "import"))
                    && tokens
                        .get(1)
                        .is_some_and(|token| is_atom(token, "function"))
                    && let Some(signature) = signature_reference_refs(&tokens)
                {
                    functions.insert(symbol, signature);
                }
            }
            TextItemKind::Variants | TextItemKind::Dispatch => {
                let members = references.into_iter().skip(1).collect::<Vec<_>>();
                sets.insert(symbol, members);
            }
            _ => {}
        }
    }
    (functions, sets)
}

fn resolve_signature_name(
    root: &str,
    functions: &HashMap<String, String>,
    sets: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let mut pending = vec![root.to_owned()];
    let mut visited = HashSet::new();
    while let Some(symbol) = pending.pop() {
        if !visited.insert(symbol.clone()) {
            continue;
        }
        if let Some(signature) = functions.get(&symbol) {
            return Some(signature.clone());
        }
        if let Some(members) = sets.get(&symbol) {
            pending.extend(members.iter().cloned());
        }
    }
    None
}

fn signature_reference_refs(tokens: &[&Token]) -> Option<String> {
    tokens.windows(3).find_map(|window| {
        (is_atom(window[0], "sig") && is_punctuation(window[1], '$'))
            .then(|| atom(window[2]).map(str::to_owned))
            .flatten()
    })
}

pub(super) fn opcode_end(instruction: &TextInstruction) -> usize {
    let tokens = &instruction.tokens;
    let mut start = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        start = tokens
            .iter()
            .position(|token| is_punctuation(token, '='))
            .map_or(0, |index| index + 1);
    }
    start.saturating_add(instruction.opcode.split('.').count() * 2 - 1)
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn significant(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect()
}

fn number(token: &Token) -> Option<usize> {
    usize::try_from(tir_format::numeric_literals::parse_unsigned_integer(atom(
        token,
    )?)?)
    .ok()
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_atom(token: &Token, expected: &str) -> bool {
    atom(token) == Some(expected)
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
