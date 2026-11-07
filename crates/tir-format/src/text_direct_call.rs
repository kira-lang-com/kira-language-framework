use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Deref;

use tir_diag::Span;
use tir_types::{FunctionType, Type};

use crate::text_global_assembly::ParsedGlobal;
use crate::text_signature_assembly::ParsedFunction;
use crate::types::{
    BinaryFunctionType, BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType,
    BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

#[derive(Clone)]
pub(crate) struct Callee {
    pub index: u64,
    pub signature: FunctionType,
    pub generic_parameters: usize,
}
pub(crate) struct GlobalReference {
    pub(crate) index: u64,
    pub(crate) thread_local: bool,
}

pub(crate) struct Callees {
    functions: HashMap<String, Callee>,
    pub(crate) globals: HashMap<String, GlobalReference>,
    pub(crate) enum_tag_widths: HashMap<String, u16>,
    pub(crate) struct_fields: HashMap<String, Vec<Type>>,
    pub(crate) union_fields: HashMap<String, Vec<Type>>,
    pub(crate) enum_payloads: HashMap<String, Vec<(String, Type)>>,
}

impl Deref for Callees {
    type Target = HashMap<String, Callee>;

    fn deref(&self) -> &Self::Target {
        &self.functions
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DirectCall {
    pub(crate) result: Option<String>,
    pub(crate) callee: String,
    pub(crate) args: Vec<String>,
    pub(crate) modifiers: u64,
}

pub(crate) fn index_callees(
    functions: &[ParsedFunction],
    globals: &[ParsedGlobal],
    module: &crate::TextModule,
) -> Result<Callees, TextParseError> {
    let mut result = HashMap::new();
    for (index, function) in functions.iter().enumerate() {
        let Type::Function(signature) = &function.ty else {
            unreachable!()
        };
        let callee = Callee {
            index: index as u64,
            signature: signature.clone(),
            generic_parameters: function.record.generic_parameters.len(),
        };
        if result
            .insert(function.record.name.clone(), callee)
            .is_some()
        {
            return Err(error(
                function.item,
                Span::new(0, 0),
                "duplicate function declaration name",
            ));
        }
    }
    let mut names = HashMap::new();
    for (index, global) in globals.iter().enumerate() {
        if names
            .insert(
                global.record.name.clone(),
                GlobalReference {
                    index: index as u64,
                    thread_local: global.record.thread_local,
                },
            )
            .is_some()
        {
            return Err(error(
                global.item,
                Span::new(0, 0),
                "duplicate global declaration name",
            ));
        }
    }
    Ok(Callees {
        functions: result,
        globals: names,
        enum_tag_widths: crate::text_type_assembly::enum_tag_widths(module)?,
        struct_fields: if functions
            .iter()
            .filter_map(|f| f.body.as_ref())
            .flat_map(|body| body.blocks.iter().flat_map(|block| &block.instructions))
            .any(|instruction| {
                matches!(
                    instruction.opcode.as_str(),
                    "struct.extract" | "struct.destructure" | "field.addr" | "const.aggregate"
                )
            }) {
            crate::text_type_assembly::trivial_struct_fields(module)?
        } else {
            HashMap::new()
        },
        union_fields: if functions
            .iter()
            .filter_map(|f| f.body.as_ref())
            .flat_map(|body| body.blocks.iter().flat_map(|block| &block.instructions))
            .any(|instruction| instruction.opcode == "const.aggregate")
        {
            crate::text_type_assembly::trivial_union_fields(module)?
        } else {
            HashMap::new()
        },
        enum_payloads: if functions
            .iter()
            .filter_map(|f| f.body.as_ref())
            .flat_map(|body| body.blocks.iter().flat_map(|block| &block.instructions))
            .any(|instruction| {
                matches!(
                    instruction.opcode.as_str(),
                    "enum.take" | "enum.project" | "enum.payload.addr" | "enum.inject"
                )
            }) {
            crate::text_type_assembly::trivial_enum_payloads(module)?
        } else {
            HashMap::new()
        },
    })
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<DirectCall, TextParseError> {
    if instruction.opcode != "call" {
        return Err(error(item, instruction.span, "expected direct call"));
    }
    let result = match instruction.results.as_slice() {
        [name] => Some(name.clone()),
        [] => None,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "direct call may define at most one result",
            ));
        }
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "direct call source span is invalid"))?
        .trim();
    let prefix = match &result {
        Some(result) => format!("%{result} = call "),
        None => "call ".to_owned(),
    };
    let raw = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "direct call requires @callee(arguments)",
        )
    })?;
    let (modifiers, raw) = crate::text_call_attributes::parse(raw, item, instruction.span)?;
    let raw = raw.strip_prefix('@').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "direct call requires @callee after modifiers",
        )
    })?;
    let (callee, arguments) = raw.split_once('(').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "direct call is missing argument parentheses",
        )
    })?;
    let arguments = arguments.strip_suffix(')').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "direct call argument list is not closed",
        )
    })?;
    if callee.is_empty()
        || callee.starts_with('.')
        || callee.ends_with('.')
        || !callee
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_'))
    {
        return Err(error(item, instruction.span, "callee name is invalid"));
    }
    let args = if arguments.trim().is_empty() {
        Vec::new()
    } else {
        arguments
            .split(',')
            .map(|argument| {
                let name = argument
                    .trim()
                    .strip_prefix('%')
                    .filter(|name| {
                        !name.is_empty()
                            && name
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                    })
                    .ok_or_else(|| {
                        error(item, instruction.span, "call argument must be an SSA value")
                    })?;
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>, TextParseError>>()?
    };
    Ok(DirectCall {
        result,
        callee: callee.to_owned(),
        args,
        modifiers,
    })
}

pub(crate) fn parse_tail(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<DirectCall, TextParseError> {
    if instruction.opcode != "tail.call" || !instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "tail.call must be a zero-result terminator",
        ));
    }
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "tail.call source span is invalid"))?
        .trim();
    let raw = line.strip_prefix("tail.call ").ok_or_else(|| {
        error(
            item,
            instruction.span,
            "tail.call requires a callee function symbol",
        )
    })?;
    let (modifiers, raw) = crate::text_call_attributes::parse(raw, item, instruction.span)?;
    let raw = raw.strip_prefix('@').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "tail.call requires @callee after modifiers",
        )
    })?;
    let (callee, values) = raw.split_once('(').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "tail.call argument parentheses are missing",
        )
    })?;
    let values = values.strip_suffix(')').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "tail.call argument list is not closed",
        )
    })?;
    if callee.is_empty()
        || callee.starts_with('.')
        || callee.ends_with('.')
        || !callee
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
    {
        return Err(error(
            item,
            instruction.span,
            "tail.call callee name is invalid",
        ));
    }
    let args = if values.trim().is_empty() {
        Vec::new()
    } else {
        values
            .split(',')
            .map(|value| {
                let name = value
                    .trim()
                    .strip_prefix('%')
                    .filter(|name| {
                        !name.is_empty()
                            && name
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                    })
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "tail.call requires SSA value arguments",
                        )
                    })?;
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>, TextParseError>>()?
    };
    Ok(DirectCall {
        result: None,
        callee: callee.to_owned(),
        args,
        modifiers,
    })
}

pub(crate) fn validate_tail(
    call: &DirectCall,
    values: &HashMap<String, Type>,
    callees: &Callees,
    caller: &FunctionType,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let callee = callees
        .get(&call.callee)
        .ok_or_else(|| error(item, span, "tail.call callee is not declared"))?;
    let signature = &callee.signature;
    if signature.abi != caller.abi
        || signature.asynchronous
        || caller.asynchronous
        || signature.foreign
        || caller.foreign
        || signature.result != caller.result
        || signature.failure != caller.failure
    {
        return Err(error(
            item,
            span,
            "tail.call ABI, failure, and result must match caller",
        ));
    }
    if signature.parameters.len() != call.args.len() {
        return Err(error(item, span, "tail.call argument count is invalid"));
    }
    for (name, parameter) in call.args.iter().zip(&signature.parameters) {
        if values.get(name) != Some(&parameter.ty) {
            return Err(error(
                item,
                span,
                "tail.call argument is undefined or mistyped",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate(
    operation: &DirectCall,
    values: &HashMap<String, Type>,
    callees: &Callees,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let signature = callees
        .get(&operation.callee)
        .ok_or_else(|| error(item, span, "direct callee is not declared in SIGNATURES"))?;
    let function = &signature.signature;
    if function.failure.is_some() || function.asynchronous || function.foreign {
        return Err(error(
            item,
            span,
            "direct call cannot target a failing, async or foreign function in this subset",
        ));
    }
    if operation.args.len() != function.parameters.len() {
        return Err(error(
            item,
            span,
            "call argument count disagrees with callee signature",
        ));
    }
    for (actual, expected) in operation.args.iter().zip(&function.parameters) {
        if values.get(actual) != Some(&expected.ty) {
            return Err(error(
                item,
                span,
                "call argument is undefined or has an incompatible type",
            ));
        }
    }
    match (operation.result.as_ref(), function.result.as_ref()) {
        (None, Type::Unit) => Ok(None),
        (Some(name), result) if !matches!(result, Type::Unit | Type::Never) => {
            Ok(Some((name.clone(), result.clone())))
        }
        _ => Err(error(
            item,
            span,
            "direct call SSA result count disagrees with callee return type",
        )),
    }
}

pub(crate) fn encode(
    operation: &DirectCall,
    values: &[&str],
    callees: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    encode_opcode(0x1a0, operation, values, callees, bytes, item, span)
}

pub(crate) fn encode_tail(
    operation: &DirectCall,
    values: &[&str],
    callees: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    encode_opcode(0x1e7, operation, values, callees, bytes, item, span)
}

fn encode_opcode(
    opcode: u64,
    operation: &DirectCall,
    values: &[&str],
    callees: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let callee = callees
        .get(&operation.callee)
        .ok_or_else(|| error(item, span, "direct callee is undefined"))?;
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(opcode)?;
    write(operation.modifiers)?;
    write(u64::from(operation.result.is_some()))?;
    let symbol = callee.index.checked_mul(4).ok_or_else(|| {
        error(
            item,
            span,
            "callee SIGNATURES index overflows symbol encoding",
        )
    })?;
    write(symbol)?;
    write(0)?;
    write(operation.args.len() as u64)?;
    for argument in &operation.args {
        let position = values
            .iter()
            .position(|name| *name == argument)
            .ok_or_else(|| error(item, span, "direct call argument SSA value is undefined"))?;
        write((values.len() - position) as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if operation.opcode != 0x1a0 {
        return Err(malformed("unsupported direct call opcode"));
    }
    let attributes = crate::text_call_attributes::canonical(operation.modifiers)?;
    let [
        BinaryOperand::Symbol { kind: 0, index },
        BinaryOperand::Types(type_args),
        rest @ ..,
    ] = operation.operands.as_slice()
    else {
        return Err(malformed(
            "direct call requires function symbol, type list, SSA arguments",
        ));
    };
    let arguments = match rest {
        [] => Vec::new(),
        [BinaryOperand::Value(value)] => vec![*value],
        [BinaryOperand::Values(values)] if values.len() >= 2 => values.clone(),
        _ => return Err(malformed("direct call SSA argument encoding is invalid")),
    };
    if !type_args.is_empty() {
        return Err(malformed(
            "generic direct-call disassembly is not implemented",
        ));
    }
    let callee = usize::try_from(*index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .ok_or_else(|| malformed("direct call function symbol is invalid"))?;
    if !callee.generic_parameters.is_empty() {
        return Err(malformed(
            "generic direct-call disassembly is not implemented",
        ));
    }
    let Some(BinaryType::Function(function)) = types.entries.get(callee.function_type as usize)
    else {
        return Err(malformed("direct callee signature is not a function type"));
    };
    if function.foreign
        || function.error.is_some()
        || function.asynchronous
        || arguments.len() != function.parameters.len()
    {
        return Err(malformed(
            "unsupported direct call callee or argument count",
        ));
    }
    let mut args = Vec::new();
    for (argument, expected) in arguments.iter().zip(&function.parameters) {
        let (name, ty) = usize::try_from(*argument)
            .ok()
            .and_then(|index| values.get(index))
            .ok_or_else(|| malformed("direct call SSA operand is undefined"))?;
        if ty != &expected.ty {
            return Err(malformed(
                "direct call SSA argument type disagrees with function signature",
            ));
        }
        args.push(format!("%{name}"));
    }
    let result = match types.entries.get(function.result as usize) {
        Some(BinaryType::Unit) if operation.result_count == 0 => None,
        Some(BinaryType::Unit | BinaryType::Never) | None => {
            return Err(malformed(
                "direct call has unsupported return type or result count",
            ));
        }
        Some(_) if operation.result_count == 1 => Some(function.result),
        _ => return Err(malformed("direct call result count disagrees with callee")),
    };
    let args = args.join(", ");
    match result {
        Some(ty) => {
            let name = format!("v{}", values.len());
            writeln!(
                text,
                "    %{name} = call{attributes} @{}({args})",
                callee.name
            )
            .map_err(formatting_error)?;
            values.push((name, ty));
        }
        None => writeln!(text, "    call{attributes} @{}({args})", callee.name)
            .map_err(formatting_error)?,
    }
    Ok(())
}

pub(crate) fn format_tail(
    text: &mut String,
    instruction: &BinaryInstruction,
    caller: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    values: &[(String, u64)],
) -> Result<(), FormatError> {
    if instruction.opcode != 0x1e7 || instruction.result_count != 0 || !instruction.terminator {
        return Err(malformed(
            "invalid tail.call terminator modifiers or result count",
        ));
    }
    let attributes = crate::text_call_attributes::canonical(instruction.modifiers)?;
    let [
        BinaryOperand::Symbol { kind: 0, index },
        BinaryOperand::Types(type_args),
        rest @ ..,
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "tail.call must refer to a function symbol and counted operands",
        ));
    };
    if !type_args.is_empty() {
        return Err(malformed(
            "generic tail calls are not supported by this text codec",
        ));
    }
    let callee = usize::try_from(*index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .ok_or_else(|| malformed("tail.call callee is out of SIGNATURES range"))?;
    if !callee.generic_parameters.is_empty() {
        return Err(malformed("generic tail.call callee is unsupported"));
    }
    let Some(BinaryType::Function(function)) = types.entries.get(callee.function_type as usize)
    else {
        return Err(malformed("tail.call callee has no function type"));
    };
    if function.result != caller.result
        || function.error != caller.error
        || function.c_abi != caller.c_abi
        || function.foreign
        || caller.foreign
        || function.asynchronous
        || caller.asynchronous
    {
        return Err(malformed(
            "tail.call callee ABI, error and result must match caller",
        ));
    }
    let args = match rest {
        [] => Vec::new(),
        [BinaryOperand::Value(value)] => vec![*value],
        [BinaryOperand::Values(values)] if values.len() >= 2 => values.clone(),
        _ => return Err(malformed("tail.call argument encoding is invalid")),
    };
    if args.len() != function.parameters.len() {
        return Err(malformed("tail.call argument count disagrees with callee"));
    }
    let mut named = Vec::new();
    for (arg, expected) in args.iter().zip(&function.parameters) {
        let (name, ty) = usize::try_from(*arg)
            .ok()
            .and_then(|index| values.get(index))
            .ok_or_else(|| malformed("tail.call SSA operand is undefined"))?;
        if *ty != expected.ty {
            return Err(malformed("tail.call argument type disagrees with callee"));
        }
        named.push(format!("%{name}"));
    }
    writeln!(
        text,
        "tail.call{attributes} @{}({})",
        callee.name,
        named.join(", ")
    )
    .map_err(formatting_error)?;
    Ok(())
}

fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
}
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
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
