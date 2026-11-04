use std::collections::{HashMap, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction};
use tir_verify::text_successors;
use tvisa_format::{TextCallingConvention, TextSignature};

const VFY7: Code = Code::new(Dialect::Tvisa, Area::Verifier, 7);
const VFY8: Code = Code::new(Dialect::Tvisa, Area::Verifier, 8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AddressSpace {
    Cpu,
    Thread,
    Threadgroup,
}

#[derive(Clone, Debug)]
struct Slot {
    scoped: bool,
    space: AddressSpace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotOperation {
    Address,
    Start,
    End,
}

pub(super) fn verify_function(
    source: &str,
    signature: &TextSignature,
    body: &FunctionBody,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let (slots, slot_diagnostics) = parse_slots(source, body, signature);
    diagnostics.extend(slot_diagnostics);

    let mut references = Vec::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            let Some(operation) = slot_operation(&instruction.opcode) else {
                continue;
            };
            let Some(id) = slot_id_operand(instruction, &mut diagnostics) else {
                continue;
            };
            let Some(slot) = slots.get(&id) else {
                diagnostics.push(
                    Diagnostic::error(VFY7, format!("slot operand names unknown slot {id}"))
                        .with_primary_span(instruction.span),
                );
                continue;
            };
            match operation {
                SlotOperation::Start | SlotOperation::End if !slot.scoped => {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY7,
                            format!(
                                "{} requires slot {id} to have the scoped flag",
                                instruction.opcode
                            ),
                        )
                        .with_primary_span(instruction.span),
                    );
                }
                _ => {}
            }
            references.push((id, operation, instruction.span));
        }
    }

    let successors = text_successors(body);
    let is_defined = !header_tokens(source, body)
        .windows(2)
        .any(|pair| is_atom(&pair[0], "semantics") && is_atom(&pair[1], "permissive"));
    let mut scoped_slots: Vec<_> = slots.iter().filter(|(_, slot)| slot.scoped).collect();
    scoped_slots.sort_by_key(|(id, _)| **id);
    for (id, _slot) in scoped_slots {
        diagnostics.extend(verify_slot_lifetime(
            *id,
            is_defined,
            body,
            &successors,
            &references,
        ));
    }
    diagnostics
}

pub(super) fn pointer_types(
    source: &str,
    signature: &TextSignature,
    body: &FunctionBody,
) -> HashMap<usize, tir_types::Type> {
    parse_slots(source, body, signature)
        .0
        .into_iter()
        .map(|(id, slot)| {
            let space = match slot.space {
                AddressSpace::Cpu => tir_types::AddressSpace::Generic,
                AddressSpace::Thread => tir_types::AddressSpace::Thread,
                AddressSpace::Threadgroup => tir_types::AddressSpace::Threadgroup,
            };
            (id, tir_types::Type::Pointer(space))
        })
        .collect()
}

fn parse_slots(
    source: &str,
    body: &FunctionBody,
    signature: &TextSignature,
) -> (HashMap<usize, Slot>, Vec<Diagnostic>) {
    let mut slots = HashMap::new();
    let mut diagnostics = Vec::new();
    let tokens = match lex(source) {
        Ok(tokens) => tokens,
        Err(error) => {
            diagnostics.push(Diagnostic::error(VFY8, error.message).with_primary_span(error.span));
            return (slots, diagnostics);
        }
    };
    let body_open = tokens
        .iter()
        .position(|token| is_punctuation(token, '{') && token.span.end == body.span.start);
    let Some(body_open) = body_open else {
        diagnostics.push(
            Diagnostic::error(VFY8, "could not locate the function body after its header")
                .with_primary_span(body.span),
        );
        return (slots, diagnostics);
    };
    let header: Vec<_> = tokens[..body_open]
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .collect();
    let mut section = None;
    for index in 0..header.len() {
        if !is_atom(header[index], "slots") {
            continue;
        }
        if section.is_some() {
            diagnostics.push(
                Diagnostic::error(VFY8, "function has more than one slots section")
                    .with_primary_span(header[index].span),
            );
            continue;
        }
        if !header
            .get(index + 1)
            .is_some_and(|token| is_punctuation(token, '{'))
        {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected '{' after slots")
                    .with_primary_span(header[index].span),
            );
            continue;
        }
        let open = index + 1;
        let Some(close) = matching_brace(&header, open) else {
            diagnostics.push(
                Diagnostic::error(VFY8, "unterminated slots section")
                    .with_primary_span(header[open].span),
            );
            continue;
        };
        section = Some((open + 1, close));
    }
    let Some((start, end)) = section else {
        return (slots, diagnostics);
    };

    let is_gpu = matches!(
        signature.convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    );
    let is_entry = signature.convention == TextCallingConvention::Entry;
    let records = &header[start..end];
    let mut cursor = 0;
    let mut record_ids = HashSet::new();
    while cursor < records.len() {
        let record_start = cursor;
        let Some(id_token) = records.get(cursor).copied() else {
            break;
        };
        let Some(id) = decimal_usize(id_token) else {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected a decimal slot number")
                    .with_primary_span(id_token.span),
            );
            break;
        };
        let unique_id = record_ids.insert(id);
        if !unique_id {
            diagnostics.push(
                Diagnostic::error(VFY8, format!("duplicate slot number {id}"))
                    .with_primary_span(id_token.span),
            );
        }
        cursor += 1;
        if !records
            .get(cursor)
            .is_some_and(|token| is_punctuation(token, ':'))
        {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected ':' after slot number").with_primary_span(
                    records
                        .get(cursor)
                        .map_or(id_token.span, |token| token.span),
                ),
            );
            break;
        }
        cursor += 1;
        if !records
            .get(cursor)
            .is_some_and(|token| is_atom(token, "size"))
        {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected 'size' in slot record").with_primary_span(
                    records
                        .get(cursor)
                        .map_or(id_token.span, |token| token.span),
                ),
            );
            break;
        }
        cursor += 1;
        let Some(size_token) = records.get(cursor).copied() else {
            diagnostics.push(
                Diagnostic::error(VFY8, "slot record is missing its size")
                    .with_primary_span(id_token.span),
            );
            break;
        };
        let Some(size) = decimal_u64(size_token) else {
            diagnostics.push(
                Diagnostic::error(VFY8, "slot size must be a decimal integer")
                    .with_primary_span(size_token.span),
            );
            break;
        };
        if size >= (1_u64 << 31) {
            diagnostics.push(
                Diagnostic::error(VFY8, "slot size must be less than 2^31 bytes")
                    .with_primary_span(size_token.span),
            );
        }
        cursor += 1;
        if !records
            .get(cursor)
            .is_some_and(|token| is_atom(token, "align"))
        {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected 'align' in slot record").with_primary_span(
                    records
                        .get(cursor)
                        .map_or(id_token.span, |token| token.span),
                ),
            );
            break;
        }
        cursor += 1;
        let Some(align_token) = records.get(cursor).copied() else {
            diagnostics.push(
                Diagnostic::error(VFY8, "slot record is missing its alignment")
                    .with_primary_span(id_token.span),
            );
            break;
        };
        let Some(align) = decimal_u64(align_token) else {
            diagnostics.push(
                Diagnostic::error(VFY8, "slot alignment must be a decimal integer")
                    .with_primary_span(align_token.span),
            );
            break;
        };
        if align == 0 || align > 4096 || !align.is_power_of_two() {
            diagnostics.push(
                Diagnostic::error(
                    VFY8,
                    "slot alignment must be a power of two from 1 through 4096 bytes",
                )
                .with_primary_span(align_token.span),
            );
        }
        cursor += 1;

        let mut space = if is_gpu {
            AddressSpace::Thread
        } else {
            AddressSpace::Cpu
        };
        if let Some(space_token) = records.get(cursor).copied()
            && matches!(space_token.kind, TokenKind::Atom(_))
            && !is_record_start(records, cursor)
        {
            match atom(space_token) {
                Some("generic") => space = AddressSpace::Cpu,
                Some("thread") => space = AddressSpace::Thread,
                Some("threadgroup") => space = AddressSpace::Threadgroup,
                Some(name) => diagnostics.push(
                    Diagnostic::error(VFY8, format!("unsupported slot address space '{name}'"))
                        .with_primary_span(space_token.span),
                ),
                None => unreachable!(),
            }
            cursor += 1;
        }
        match space {
            AddressSpace::Cpu if is_gpu => diagnostics.push(
                Diagnostic::error(VFY8, "GPU frame slots must use thread or threadgroup space")
                    .with_primary_span(id_token.span),
            ),
            AddressSpace::Thread | AddressSpace::Threadgroup if !is_gpu => diagnostics.push(
                Diagnostic::error(VFY8, "CPU frame slots must use generic stack space")
                    .with_primary_span(id_token.span),
            ),
            AddressSpace::Threadgroup if !is_entry => diagnostics.push(
                Diagnostic::error(
                    VFY8,
                    "threadgroup frame slots are only valid in entry functions",
                )
                .with_primary_span(id_token.span),
            ),
            _ => {}
        }

        let mut scoped = false;
        if records
            .get(cursor)
            .is_some_and(|token| is_punctuation(token, '['))
        {
            let flags_open = cursor;
            cursor += 1;
            let mut seen = HashSet::new();
            let mut need_flag = true;
            loop {
                let Some(token) = records.get(cursor).copied() else {
                    diagnostics.push(
                        Diagnostic::error(VFY8, "unterminated slot flag list")
                            .with_primary_span(records[flags_open].span),
                    );
                    break;
                };
                if is_punctuation(token, ']') {
                    if need_flag {
                        diagnostics.push(
                            Diagnostic::error(
                                VFY8,
                                "slot flag list must not be empty or end with a comma",
                            )
                            .with_primary_span(token.span),
                        );
                    }
                    cursor += 1;
                    break;
                }
                if !need_flag || !matches!(token.kind, TokenKind::Atom(_)) {
                    diagnostics.push(
                        Diagnostic::error(VFY8, "expected a slot flag or ']' in slot flag list")
                            .with_primary_span(token.span),
                    );
                    break;
                }
                let Some(flag) = atom(token) else {
                    unreachable!();
                };
                if !matches!(
                    flag,
                    "escapes" | "zeroinit" | "protected" | "tagged" | "scoped"
                ) {
                    diagnostics.push(
                        Diagnostic::error(VFY8, format!("unknown slot flag '{flag}'"))
                            .with_primary_span(token.span),
                    );
                }
                if !seen.insert(flag.to_owned()) {
                    diagnostics.push(
                        Diagnostic::error(VFY8, format!("duplicate slot flag '{flag}'"))
                            .with_primary_span(token.span),
                    );
                }
                scoped |= flag == "scoped";
                cursor += 1;
                need_flag = false;
                if records
                    .get(cursor)
                    .is_some_and(|next| is_punctuation(next, ','))
                {
                    cursor += 1;
                    need_flag = true;
                } else if records
                    .get(cursor)
                    .is_some_and(|next| is_punctuation(next, ']'))
                {
                    // The closing bracket is consumed by the next loop iteration.
                } else if cursor < records.len() {
                    diagnostics.push(
                        Diagnostic::error(VFY8, "expected ',' or ']' after slot flag")
                            .with_primary_span(records[cursor].span),
                    );
                    break;
                }
            }
        }

        if unique_id
            && size < (1_u64 << 31)
            && align != 0
            && align <= 4096
            && align.is_power_of_two()
        {
            slots.insert(id, Slot { scoped, space });
        }

        if cursor == record_start {
            break;
        }
        if cursor < records.len() && !is_record_start(records, cursor) {
            diagnostics.push(
                Diagnostic::error(VFY8, "expected the next slot record")
                    .with_primary_span(records[cursor].span),
            );
            break;
        }
    }
    (slots, diagnostics)
}

fn verify_slot_lifetime(
    id: usize,
    is_defined: bool,
    body: &FunctionBody,
    successors: &[Vec<usize>],
    references: &[(usize, SlotOperation, Span)],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut operation_at = HashMap::new();
    for (slot_id, operation, span) in references {
        if *slot_id == id {
            operation_at.insert((span.start, span.end), *operation);
        }
    }
    let mut reported = HashSet::new();
    let mut seen = vec![[false; 2]; body.blocks.len()];
    let mut queue = VecDeque::new();
    for root in 0..body.blocks.len() {
        if seen[root][0] || seen[root][1] {
            continue;
        }
        seen[root][0] = true;
        queue.push_back((root, false));
        while let Some((block_index, mut active)) = queue.pop_front() {
            let block = &body.blocks[block_index];
            for instruction in &block.instructions {
                let operation = operation_at.get(&(instruction.span.start, instruction.span.end));
                match operation {
                    Some(SlotOperation::Start) => {
                        if active && reported.insert((instruction.span.start, 0_u8)) {
                            diagnostics.push(
                                Diagnostic::error(
                                    VFY7,
                                    format!(
                                        "slot.start {id} occurs while its scoped lifetime is active"
                                    ),
                                )
                                .with_primary_span(instruction.span),
                            );
                        }
                        active = true;
                    }
                    Some(SlotOperation::End) => {
                        if !active && reported.insert((instruction.span.start, 1_u8)) {
                            diagnostics.push(
                                Diagnostic::error(
                                    VFY7,
                                    format!(
                                        "slot.end {id} occurs while its scoped lifetime is inactive"
                                    ),
                                )
                                .with_primary_span(instruction.span),
                            );
                        }
                        active = false;
                    }
                    Some(SlotOperation::Address) if is_defined && !active => {
                        if reported.insert((instruction.span.start, 2_u8)) {
                            diagnostics.push(
                                Diagnostic::error(
                                    VFY7,
                                    format!("slot.addr {id} occurs outside its scoped lifetime"),
                                )
                                .with_primary_span(instruction.span),
                            );
                        }
                    }
                    _ => {}
                }
            }
            let targets = successors.get(block_index).map_or(&[][..], Vec::as_slice);
            if targets.is_empty() && active && reported.insert((block.span.start, 3_u8)) {
                diagnostics.push(
                    Diagnostic::error(
                        VFY7,
                        format!("scoped slot {id} remains active at function exit"),
                    )
                    .with_primary_span(block.span),
                );
            }
            for &target in targets {
                if target >= seen.len() {
                    continue;
                }
                let state = usize::from(active);
                if !seen[target][state] {
                    seen[target][state] = true;
                    queue.push_back((target, active));
                }
            }
        }
    }
    diagnostics
}

fn slot_operation(opcode: &str) -> Option<SlotOperation> {
    match opcode {
        "slot.addr" => Some(SlotOperation::Address),
        "slot.start" => Some(SlotOperation::Start),
        "slot.end" => Some(SlotOperation::End),
        _ => None,
    }
}

fn slot_id_operand(
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let start = instruction_operands_start(&instruction.tokens);
    let Some(token) = instruction.tokens.get(start) else {
        diagnostics.push(
            Diagnostic::error(
                VFY7,
                format!("{} is missing its slot number", instruction.opcode),
            )
            .with_primary_span(instruction.span),
        );
        return None;
    };
    let Some(id) = decimal_usize(token) else {
        diagnostics.push(
            Diagnostic::error(
                VFY7,
                format!("{} requires a decimal slot number", instruction.opcode),
            )
            .with_primary_span(token.span),
        );
        return None;
    };
    Some(id)
}

fn instruction_operands_start(tokens: &[Token]) -> usize {
    let mut cursor = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        loop {
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            ) || !matches!(
                tokens.get(cursor + 1).map(|token| &token.kind),
                Some(TokenKind::Atom(_))
            ) {
                return 0;
            }
            cursor += 2;
            match tokens.get(cursor).map(|token| &token.kind) {
                Some(TokenKind::Punctuation('=')) => {
                    cursor += 1;
                    break;
                }
                Some(TokenKind::Punctuation(',')) => cursor += 1,
                _ => return 0,
            }
        }
    }
    cursor += 1;
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) && matches!(
        tokens.get(cursor + 1).map(|token| &token.kind),
        Some(TokenKind::Atom(_))
    ) {
        cursor += 2;
    }
    cursor
}

fn header_tokens(source: &str, body: &FunctionBody) -> Vec<Token> {
    let Ok(tokens) = lex(source) else {
        return Vec::new();
    };
    let end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{') && token.span.end == body.span.start)
        .unwrap_or(tokens.len());
    tokens[..end]
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .cloned()
        .collect()
}

fn is_record_start(tokens: &[&Token], index: usize) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| decimal_usize(token).is_some())
        && tokens
            .get(index + 1)
            .is_some_and(|token| is_punctuation(token, ':'))
}

fn decimal_usize(token: &Token) -> Option<usize> {
    decimal_u64(token).and_then(|value| usize::try_from(value).ok())
}

fn decimal_u64(token: &Token) -> Option<u64> {
    let TokenKind::Atom(value) = &token.kind else {
        return None;
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn matching_brace(tokens: &[&Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if is_punctuation(token, '{') {
            depth += 1;
        } else if is_punctuation(token, '}') {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn is_atom(token: &Token, expected: &str) -> bool {
    matches!(&token.kind, TokenKind::Atom(value) if value == expected)
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
