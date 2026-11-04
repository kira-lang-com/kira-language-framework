use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction, numeric_literals};
use tir_types::Type;

use crate::{DominatorTree, text_cfg};

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);

struct Attachment<'a> {
    name: String,
    span: Span,
    arguments: Vec<&'a [Token]>,
}

pub(super) fn verify(
    body: &FunctionBody,
    value_types: &std::collections::HashMap<String, Type>,
) -> Vec<Diagnostic> {
    let successors = text_cfg::text_successors(body);
    let dominators = DominatorTree::compute(&successors);
    let mut diagnostics = Vec::new();

    for (block_index, block) in body.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            let mut seen = HashSet::new();
            for attachment in attachments(&instruction.tokens) {
                if matches!(attachment.name.as_str(), "var" | "label") {
                    continue;
                }
                if !known_metadata(&attachment.name) {
                    diagnostics.push(error(
                        attachment.span,
                        format!("unsupported TIR metadata !{}", attachment.name),
                    ));
                    continue;
                }
                if !seen.insert(attachment.name.clone()) {
                    diagnostics.push(error(
                        attachment.span,
                        format!(
                            "metadata !{} occurs more than once on an instruction",
                            attachment.name
                        ),
                    ));
                    continue;
                }
                verify_attachment(
                    &mut diagnostics,
                    body,
                    block_index,
                    instruction,
                    &attachment,
                    value_types,
                    &dominators,
                );
            }
        }
    }
    diagnostics
}

fn known_metadata(name: &str) -> bool {
    matches!(
        name,
        "loc"
            | "branch_weights"
            | "loop"
            | "range"
            | "nonnull"
            | "align"
            | "noalias"
            | "alias.scope"
            | "tbaa"
            | "profile_id"
            | "invariant.load"
    )
}

fn verify_attachment(
    diagnostics: &mut Vec<Diagnostic>,
    body: &FunctionBody,
    block_index: usize,
    instruction: &TextInstruction,
    attachment: &Attachment<'_>,
    value_types: &std::collections::HashMap<String, Type>,
    dominators: &DominatorTree,
) {
    let arguments = &attachment.arguments;
    match attachment.name.as_str() {
        "loc" => verify_location(diagnostics, attachment),
        "branch_weights" => {
            if arguments.len() < 2
                || !arguments
                    .iter()
                    .all(|argument| u64_value(argument).is_some())
            {
                diagnostics.push(error(
                    attachment.span,
                    "!branch_weights requires at least two unsigned 64-bit weights",
                ));
            }
            if text_cfg::conditional_successor_count(instruction) != Some(arguments.len()) {
                diagnostics.push(error(
                    attachment.span,
                    "!branch_weights requires one weight per conditional-branch or integer-switch successor",
                ));
            }
        }
        "loop" => {
            let values = arguments
                .iter()
                .take(3)
                .map(|argument| u64_value(argument))
                .collect::<Option<Vec<_>>>();
            let must_progress = arguments.get(3).and_then(|argument| boolean(argument));
            let valid_shape = arguments.len() == 4 && values.is_some() && must_progress.is_some();
            if !valid_shape {
                diagnostics.push(error(
                    attachment.span,
                    "!loop requires three unsigned counts and a boolean mustprogress value",
                ));
            } else if values
                .as_ref()
                .is_some_and(|values| values.iter().all(|value| *value == 0))
                && must_progress == Some(false)
            {
                diagnostics.push(error(
                    attachment.span,
                    "!loop must contain at least one hint",
                ));
            }
            let has_backedge = text_cfg::is_control_terminator(&instruction.opcode)
                && text_cfg::instruction_targets(body, instruction)
                    .into_iter()
                    .any(|target| {
                        target < body.blocks.len()
                            && dominators.dominates(target, block_index)
                            && is_loop_header(&body.blocks[target].header_tokens)
                    });
            if !has_backedge {
                diagnostics.push(error(
                    attachment.span,
                    "!loop must be attached to a terminator with an edge to a dominating loop header",
                ));
            }
        }
        "range" => verify_range(diagnostics, instruction, attachment, value_types),
        "nonnull" => {
            if !arguments.is_empty() {
                diagnostics.push(error(attachment.span, "!nonnull takes no values"));
            }
            if !is_load(&instruction.opcode)
                || !result_type(instruction, value_types).is_some_and(is_pointer_type)
            {
                diagnostics.push(error(
                    attachment.span,
                    "!nonnull requires a pointer-valued load",
                ));
            }
        }
        "align" => {
            let alignment = arguments.first().and_then(|argument| u64_value(argument));
            if arguments.len() != 1
                || !alignment.is_some_and(|value| value != 0 && value.is_power_of_two())
            {
                diagnostics.push(error(
                    attachment.span,
                    "!align requires one positive power-of-two byte alignment",
                ));
            }
            if !is_load(&instruction.opcode)
                || !result_type(instruction, value_types).is_some_and(is_pointer_type)
            {
                diagnostics.push(error(
                    attachment.span,
                    "!align requires a pointer-valued load",
                ));
            }
        }
        "noalias" | "alias.scope" => verify_alias_scopes(diagnostics, instruction, attachment),
        "tbaa" => {
            if !is_single_nonempty_string(arguments) {
                diagnostics.push(error(
                    attachment.span,
                    "!tbaa requires one nonempty string tag",
                ));
            }
            if !is_memory_operation(&instruction.opcode) {
                diagnostics.push(error(
                    attachment.span,
                    "!tbaa must be attached to a memory operation",
                ));
            }
        }
        "profile_id" => {
            if arguments.len() != 1
                || !arguments
                    .first()
                    .and_then(|argument| unsigned_value(argument))
                    .is_some_and(|value| value < (1_u128 << 32))
            {
                diagnostics.push(error(
                    attachment.span,
                    "!profile_id requires one unsigned value below 2^32",
                ));
            }
        }
        "invariant.load" => {
            if !arguments.is_empty() {
                diagnostics.push(error(attachment.span, "!invariant.load takes no values"));
            }
            if !is_load(&instruction.opcode) {
                diagnostics.push(error(
                    attachment.span,
                    "!invariant.load must be attached to a load",
                ));
            }
        }
        _ => {}
    }
}

fn verify_location(diagnostics: &mut Vec<Diagnostic>, attachment: &Attachment<'_>) {
    let valid = match attachment.arguments.as_slice() {
        [tokens] => matches!(
            tokens,
            [path, colon1, line, colon2, column]
                if string_value(std::slice::from_ref(path)).is_some()
                    && punctuation(Some(colon1), ':')
                    && positive_u64(std::slice::from_ref(line))
                    && punctuation(Some(colon2), ':')
                    && positive_u64(std::slice::from_ref(column))
        ),
        _ => false,
    };
    if !valid {
        diagnostics.push(error(
            attachment.span,
            "!loc requires a valid escaped path and positive 64-bit line and column values",
        ));
    }
}

fn verify_range(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    attachment: &Attachment<'_>,
    value_types: &std::collections::HashMap<String, Type>,
) {
    if !is_load(&instruction.opcode) {
        diagnostics.push(error(attachment.span, "!range must be attached to a load"));
    }
    let Some(Type::Integer(bits)) = result_type(instruction, value_types) else {
        diagnostics.push(error(
            attachment.span,
            "!range requires an integer-valued load",
        ));
        return;
    };
    if attachment.arguments.is_empty() || attachment.arguments.len() % 2 != 0 {
        diagnostics.push(error(
            attachment.span,
            "!range requires one or more lower and upper bound pairs",
        ));
        return;
    }
    let max_value = if *bits >= 128 {
        u128::MAX
    } else if *bits == 0 {
        0
    } else {
        (1_u128 << *bits) - 1
    };
    let mut previous_upper = None;
    for pair in attachment.arguments.chunks_exact(2) {
        let (Some(lower), Some(upper)) = (unsigned_value(pair[0]), unsigned_value(pair[1])) else {
            diagnostics.push(error(
                attachment.span,
                "!range bounds must be unsigned integer literals",
            ));
            return;
        };
        if lower > max_value || upper > max_value || lower >= upper {
            diagnostics.push(error(
                attachment.span,
                "!range bounds must be ordered values in the integer result type",
            ));
            return;
        }
        if previous_upper.is_some_and(|previous| lower < previous) {
            diagnostics.push(error(
                attachment.span,
                "!range intervals must be ordered and disjoint",
            ));
            return;
        }
        previous_upper = Some(upper);
    }
}

fn verify_alias_scopes(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    attachment: &Attachment<'_>,
) {
    if attachment.arguments.is_empty() || attachment.arguments.len() % 2 != 0 {
        diagnostics.push(error(
            attachment.span,
            "alias metadata requires one or more domain and scope string pairs",
        ));
        return;
    }
    let mut pairs = HashSet::new();
    for pair in attachment.arguments.chunks_exact(2) {
        let (Some(domain), Some(scope)) = (string_value(pair[0]), string_value(pair[1])) else {
            diagnostics.push(error(
                attachment.span,
                "alias metadata domain and scope names must be valid strings",
            ));
            return;
        };
        if domain.is_empty() || scope.is_empty() || !pairs.insert((domain, scope)) {
            diagnostics.push(error(
                attachment.span,
                "alias metadata names must be nonempty and pairs must be unique",
            ));
            return;
        }
    }
    if !is_memory_operation(&instruction.opcode) {
        diagnostics.push(error(
            attachment.span,
            "alias metadata must be attached to a memory operation",
        ));
    }
}

fn is_single_nonempty_string(arguments: &[&[Token]]) -> bool {
    arguments.len() == 1 && string_value(arguments[0]).is_some_and(|value| !value.is_empty())
}

fn result_type<'a>(
    instruction: &TextInstruction,
    value_types: &'a std::collections::HashMap<String, Type>,
) -> Option<&'a Type> {
    instruction
        .results
        .first()
        .and_then(|result| value_types.get(result))
}

fn is_pointer_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Pointer(_) | Type::Reference(_) | Type::Unique(_) | Type::Weak(_)
    )
}

fn is_load(opcode: &str) -> bool {
    matches!(
        opcode,
        "load" | "load.borrow" | "raw.load" | "atomic.load" | "mmio.load"
    )
}

fn is_memory_operation(opcode: &str) -> bool {
    matches!(
        opcode,
        "load"
            | "load.borrow"
            | "store"
            | "copy.addr"
            | "move.addr"
            | "destroy.addr"
            | "raw.load"
            | "raw.store"
            | "mem.copy"
            | "mem.move"
            | "mem.set"
            | "atomic.load"
            | "atomic.store"
            | "atomic.rmw"
            | "atomic.cmpxchg"
            | "mmio.load"
            | "mmio.store"
    )
}

fn is_loop_header(tokens: &[Token]) -> bool {
    tokens.windows(4).any(|window| {
        punctuation(window.first(), ':')
            && atom(&window[1]) == Some("loop")
            && punctuation(window.get(2), '(')
            && atom(&window[3]) == Some("merge")
    })
}

fn positive_u64(tokens: &[Token]) -> bool {
    u64_value(tokens).is_some_and(|value| value > 0)
}

fn u64_value(tokens: &[Token]) -> Option<u64> {
    u64::try_from(unsigned_value(tokens)?).ok()
}

fn unsigned_value(tokens: &[Token]) -> Option<u128> {
    let [token] = tokens else {
        return None;
    };
    atom(token).and_then(numeric_literals::parse_unsigned_integer)
}

fn boolean(tokens: &[Token]) -> Option<bool> {
    let [token] = tokens else {
        return None;
    };
    match atom(token)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn string_value(tokens: &[Token]) -> Option<String> {
    let [
        Token {
            kind: TokenKind::String(raw),
            ..
        },
    ] = tokens
    else {
        return None;
    };
    tir_format::text::decode_kira_string_literal_contents(raw).ok()
}

fn attachments(tokens: &[Token]) -> Vec<Attachment<'_>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        if !punctuation(tokens.get(cursor), '!') {
            cursor += 1;
            continue;
        }
        let span = tokens[cursor].span;
        let Some(first) = tokens.get(cursor + 1).and_then(atom) else {
            break;
        };
        let mut name = first.to_owned();
        let mut name_end = cursor + 1;
        while punctuation(tokens.get(name_end + 1), '.') {
            let Some(component) = tokens.get(name_end + 2).and_then(atom) else {
                break;
            };
            name.push('.');
            name.push_str(component);
            name_end += 2;
        }
        let open = name_end + 1;
        if !punctuation(tokens.get(open), '(') {
            break;
        }
        let Some(close) = matching_parenthesis(tokens, open) else {
            break;
        };
        output.push(Attachment {
            name,
            span,
            arguments: split_arguments(&tokens[open + 1..close]),
        });
        cursor = close + 1;
    }
    output
}

fn split_arguments(tokens: &[Token]) -> Vec<&[Token]> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Punctuation('(' | '[' | '{' | '<') => depth += 1,
            TokenKind::Punctuation(')' | ']' | '}' | '>') => depth = depth.saturating_sub(1),
            TokenKind::Punctuation(',') if depth == 0 => {
                arguments.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    arguments.push(&tokens[start..]);
    arguments
}

fn matching_parenthesis(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: Option<&Token>, expected: char) -> bool {
    token.is_some_and(
        |token| matches!(token.kind, TokenKind::Punctuation(value) if value == expected),
    )
}

fn error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(V1, message).with_primary_span(span)
}
