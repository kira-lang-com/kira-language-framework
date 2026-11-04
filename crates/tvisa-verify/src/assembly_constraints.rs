use std::collections::HashSet;

use tir_format::text::{Token, TokenKind, decode_kira_string_literal_contents};
use tir_format::{StringTable, TextInstruction};
use tvisa_format::{AssemblyRecord, BinaryInstruction, BinaryOperand};

use tvisa_format::TextVariantArchitecture as VariantArchitecture;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum AssemblyArchitecture {
    Aarch64,
    X64,
    Rv64,
}

impl AssemblyArchitecture {
    pub(super) const fn from_variant(architecture: VariantArchitecture) -> Option<Self> {
        match architecture {
            VariantArchitecture::Aarch64 => Some(Self::Aarch64),
            VariantArchitecture::X64 => Some(Self::X64),
            VariantArchitecture::Rv64 => Some(Self::Rv64),
            VariantArchitecture::Portable | VariantArchitecture::Wasm => None,
        }
    }

    pub(super) const fn from_code(architecture: u8) -> Option<Self> {
        match architecture {
            1 => Some(Self::Aarch64),
            2 => Some(Self::X64),
            3 => Some(Self::Rv64),
            _ => None,
        }
    }
}

pub(super) fn verify_constraints(
    architecture: AssemblyArchitecture,
    constraints: &str,
    input_count: usize,
    output_count: usize,
) -> Result<(), String> {
    if constraints.is_empty() {
        return arity(input_count, output_count, 0, 0);
    }
    if constraints.chars().any(char::is_whitespace) {
        return Err("assembly constraints cannot contain whitespace".to_owned());
    }

    let entries = constraints.split(',').collect::<Vec<_>>();
    if entries.iter().any(|entry| entry.is_empty()) {
        return Err("assembly constraint list contains an empty entry".to_owned());
    }

    let mut outputs = Vec::new();
    let mut input_operands = 0usize;
    let mut tied_outputs = HashSet::new();
    let mut clobbers = HashSet::new();
    let mut alternatives = None;
    let mut saw_input = false;
    let mut saw_clobber = false;

    for entry in entries {
        if let Some(register) = entry
            .strip_prefix("~{")
            .and_then(|entry| entry.strip_suffix('}'))
        {
            saw_clobber = true;
            validate_clobber(architecture, register, &mut clobbers)?;
            continue;
        }
        if entry.starts_with('~') {
            return Err(format!("invalid clobber constraint {entry:?}"));
        }
        if saw_clobber {
            return Err("clobber constraints must follow outputs and inputs".to_owned());
        }

        let (output, read_write, code) = if let Some(code) = entry.strip_prefix('=') {
            (true, false, code)
        } else if let Some(code) = entry.strip_prefix('+') {
            (true, true, code)
        } else {
            (false, false, entry)
        };

        if output {
            if saw_input {
                return Err("output constraints must precede input constraints".to_owned());
            }
            let code = code.strip_prefix('&').unwrap_or(code);
            if code.is_empty() || code.contains('&') {
                return Err(format!("invalid output constraint {entry:?}"));
            }
            let (count, register_output) = validate_code(architecture, code, true)?;
            validate_alternative_count(&mut alternatives, count)?;
            outputs.push(register_output);
            if read_write {
                input_operands = input_operands.saturating_add(1);
            }
            continue;
        }

        saw_input = true;
        let tied_indices = code
            .split('|')
            .map(str::parse::<usize>)
            .collect::<Result<Vec<_>, _>>();
        if let Ok(tied_indices) = tied_indices {
            validate_alternative_count(&mut alternatives, tied_indices.len())?;
            for output_index in tied_indices {
                if !outputs.get(output_index).copied().unwrap_or(false)
                    || !tied_outputs.insert(output_index)
                {
                    return Err(format!("invalid or duplicate tie to output {output_index}"));
                }
            }
        } else {
            let (count, _) = validate_code(architecture, code, false)?;
            validate_alternative_count(&mut alternatives, count)?;
        }
        input_operands = input_operands.saturating_add(1);
    }

    if outputs.len() != output_count {
        return Err(format!(
            "constraint list has {} outputs, instruction produces {output_count}",
            outputs.len()
        ));
    }
    arity(input_count, output_count, input_operands, outputs.len())
}

pub(super) fn verify_text_instruction(
    instruction: &TextInstruction,
    architecture: AssemblyArchitecture,
) -> Result<(), String> {
    let Some((template, constraints, input_count)) =
        text_constraints_and_inputs(&instruction.tokens)
    else {
        return Err(
            "assembly instruction is missing its template, constraints, or operands".to_owned(),
        );
    };
    decode_kira_string_literal_contents(template)
        .map_err(|error| format!("invalid assembly template string escape: {error}"))?;
    let constraints = decode_kira_string_literal_contents(constraints)
        .map_err(|error| format!("invalid assembly string escape: {error}"))?;
    verify_constraints(
        architecture,
        &constraints,
        input_count,
        instruction.results.len(),
    )
}

pub(super) fn verify_binary_instruction(
    instruction: &BinaryInstruction,
    architecture: AssemblyArchitecture,
    records: &[AssemblyRecord],
    strings: &StringTable,
) -> Result<(), String> {
    let record_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
        .ok_or_else(|| "assembly instruction has no ASM record index".to_owned())?;
    let record = usize::try_from(record_index)
        .ok()
        .and_then(|index| records.get(index))
        .ok_or_else(|| "assembly instruction has an invalid ASM record index".to_owned())?;
    let constraints = usize::try_from(record.constraints_index)
        .ok()
        .and_then(|index| strings.get(index))
        .ok_or_else(|| "ASM constraint string index is invalid".to_owned())?;
    let input_count = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.len()),
            BinaryOperand::Value(_) => Some(1),
            _ => None,
        })
        .unwrap_or(0);
    verify_constraints(
        architecture,
        constraints,
        input_count,
        usize::from(instruction.result_count),
    )
}

fn text_constraints_and_inputs(tokens: &[Token]) -> Option<(&str, &str, usize)> {
    let mut strings = tokens.iter().filter_map(|token| match &token.kind {
        TokenKind::String(value) => Some(value.as_str()),
        _ => None,
    });
    let template = strings.next()?;
    let constraints = strings.next()?;
    let constraints_end = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| matches!(token.kind, TokenKind::String(_)).then_some(index))
        .nth(1)?;
    let open = tokens
        .iter()
        .enumerate()
        .skip(constraints_end + 1)
        .find(|(_, token)| matches!(token.kind, TokenKind::Punctuation('(')))?
        .0;
    let mut parens = 0usize;
    let mut angles = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    let mut has_argument = false;
    let mut count = 1usize;
    for token in tokens.iter().skip(open) {
        match token.kind {
            TokenKind::Punctuation('(') => parens += 1,
            TokenKind::Punctuation(')') => {
                parens = parens.saturating_sub(1);
                if parens == 0 {
                    return Some((template, constraints, if has_argument { count } else { 0 }));
                }
            }
            TokenKind::Punctuation('<') => angles += 1,
            TokenKind::Punctuation('>') => angles = angles.saturating_sub(1),
            TokenKind::Punctuation('[') => brackets += 1,
            TokenKind::Punctuation(']') => brackets = brackets.saturating_sub(1),
            TokenKind::Punctuation('{') => braces += 1,
            TokenKind::Punctuation('}') => braces = braces.saturating_sub(1),
            TokenKind::Punctuation(',')
                if parens == 1 && angles == 0 && brackets == 0 && braces == 0 =>
            {
                count += 1;
            }
            TokenKind::Newline | TokenKind::End => {}
            _ => has_argument = true,
        }
    }
    None
}

fn validate_code(
    architecture: AssemblyArchitecture,
    code: &str,
    output: bool,
) -> Result<(usize, bool), String> {
    let mut alternatives = 0usize;
    let mut register_only = true;
    for alternative in code.split('|') {
        if alternative.is_empty() {
            return Err("constraint contains an empty alternative".to_owned());
        }
        alternatives += 1;
        if architecture == AssemblyArchitecture::X64 && output && is_x64_condition(alternative) {
            continue;
        }
        let codes = parse_code_set(architecture, alternative)?;
        register_only &= codes
            .iter()
            .all(|code| is_register_code(architecture, code));
    }
    Ok((alternatives, register_only))
}

fn parse_code_set(
    architecture: AssemblyArchitecture,
    alternative: &str,
) -> Result<Vec<&str>, String> {
    if alternative.starts_with("@cc") {
        return Err(format!(
            "condition-code constraint {alternative:?} is only a supported x64 output"
        ));
    }
    let codes: &[&str] = match architecture {
        AssemblyArchitecture::Aarch64 => &[
            "Upa", "Upl", "r", "w", "x", "y", "I", "J", "K", "L", "M", "N", "Q", "m", "i", "n",
        ],
        AssemblyArchitecture::X64 => &[
            "Yk", "r", "q", "Q", "a", "b", "c", "d", "S", "D", "x", "v", "k", "m", "i", "n", "e",
            "Z",
        ],
        AssemblyArchitecture::Rv64 => &["vr", "vm", "r", "f", "I", "J", "K", "A", "m", "i", "n"],
    };
    let mut cursor = 0usize;
    let mut parsed = Vec::new();
    while cursor < alternative.len() {
        let remaining = &alternative[cursor..];
        let Some(code) = codes.iter().find(|code| remaining.starts_with(**code)) else {
            return Err(format!(
                "unsupported {} constraint code in {alternative:?}",
                arch_name(architecture)
            ));
        };
        parsed.push(*code);
        cursor += code.len();
    }
    if parsed.is_empty() {
        return Err("constraint code is empty".to_owned());
    }
    Ok(parsed)
}

fn validate_alternative_count(current: &mut Option<usize>, count: usize) -> Result<(), String> {
    match current {
        Some(expected) if *expected != count => {
            Err("all constraint entries must have the same number of alternatives".to_owned())
        }
        Some(_) => Ok(()),
        None => {
            *current = Some(count);
            Ok(())
        }
    }
}

fn validate_clobber(
    architecture: AssemblyArchitecture,
    register: &str,
    clobbers: &mut HashSet<String>,
) -> Result<(), String> {
    if !matches!(register, "memory" | "cc") && !is_register_name(architecture, register) {
        return Err(format!(
            "unknown {} clobber {register:?}",
            arch_name(architecture)
        ));
    }
    if is_protected_register(architecture, register) {
        return Err(format!("clobber of protected register {register:?}"));
    }
    if !clobbers.insert(register.to_owned()) {
        return Err(format!("duplicate clobber {register:?}"));
    }
    Ok(())
}

fn is_register_code(architecture: AssemblyArchitecture, code: &str) -> bool {
    match architecture {
        AssemblyArchitecture::Aarch64 => matches!(code, "r" | "w" | "x" | "y" | "Upa" | "Upl"),
        AssemblyArchitecture::X64 => matches!(
            code,
            "r" | "q" | "Q" | "a" | "b" | "c" | "d" | "S" | "D" | "x" | "v" | "Yk" | "k"
        ),
        AssemblyArchitecture::Rv64 => matches!(code, "r" | "f" | "vr" | "vm"),
    }
}

fn is_x64_condition(code: &str) -> bool {
    let Some(condition) = code.strip_prefix("@cc") else {
        return false;
    };
    matches!(
        condition,
        "a" | "ae"
            | "b"
            | "be"
            | "c"
            | "e"
            | "g"
            | "ge"
            | "l"
            | "le"
            | "na"
            | "nae"
            | "nb"
            | "nbe"
            | "nc"
            | "ne"
            | "ng"
            | "nge"
            | "nl"
            | "nle"
            | "no"
            | "np"
            | "ns"
            | "nz"
            | "o"
            | "p"
            | "pe"
            | "po"
            | "s"
            | "z"
    )
}

fn is_register_name(architecture: AssemblyArchitecture, register: &str) -> bool {
    match architecture {
        AssemblyArchitecture::Aarch64 => {
            matches!(register, "sp" | "fp" | "lr" | "xzr" | "wzr" | "ffr")
                || numbered(register, 'x', 30)
                || numbered(register, 'w', 30)
                || numbered(register, 'v', 31)
                || numbered(register, 'q', 31)
                || numbered(register, 'd', 31)
                || numbered(register, 's', 31)
                || numbered(register, 'h', 31)
                || numbered(register, 'b', 31)
                || numbered(register, 'z', 31)
                || numbered(register, 'p', 15)
        }
        AssemblyArchitecture::X64 => {
            x64_gpr(register)
                || numbered_prefixed(register, "xmm", 31)
                || numbered_prefixed(register, "ymm", 31)
                || numbered_prefixed(register, "zmm", 31)
                || numbered_prefixed(register, "k", 7)
                || numbered_prefixed(register, "mm", 7)
                || numbered_prefixed(register, "st", 7)
                || matches!(
                    register,
                    "rip"
                        | "eip"
                        | "ip"
                        | "cs"
                        | "ds"
                        | "es"
                        | "fs"
                        | "gs"
                        | "ss"
                        | "rflags"
                        | "eflags"
                        | "dirflag"
                        | "fpsr"
                )
        }
        AssemblyArchitecture::Rv64 => {
            matches!(register, "zero" | "ra" | "sp" | "gp" | "tp" | "fp")
                || numbered(register, 'x', 31)
                || numbered(register, 'f', 31)
                || numbered(register, 'v', 31)
                || numbered_prefixed(register, "t", 6)
                || numbered_prefixed(register, "s", 11)
                || numbered_prefixed(register, "a", 7)
        }
    }
}

fn is_protected_register(architecture: AssemblyArchitecture, register: &str) -> bool {
    match architecture {
        AssemblyArchitecture::Aarch64 => {
            matches!(register, "x18" | "w18" | "x29" | "w29" | "fp" | "sp")
        }
        AssemblyArchitecture::X64 => {
            matches!(register, "rbp" | "ebp" | "bp" | "rsp" | "esp" | "sp")
        }
        AssemblyArchitecture::Rv64 => matches!(
            register,
            "gp" | "tp" | "x3" | "x4" | "fp" | "s0" | "x8" | "sp" | "x2"
        ),
    }
}

fn x64_gpr(register: &str) -> bool {
    matches!(
        register,
        "rax"
            | "eax"
            | "ax"
            | "al"
            | "ah"
            | "rbx"
            | "ebx"
            | "bx"
            | "bl"
            | "bh"
            | "rcx"
            | "ecx"
            | "cx"
            | "cl"
            | "ch"
            | "rdx"
            | "edx"
            | "dx"
            | "dl"
            | "dh"
            | "rsi"
            | "esi"
            | "si"
            | "sil"
            | "rdi"
            | "edi"
            | "di"
            | "dil"
            | "rbp"
            | "ebp"
            | "bp"
            | "bpl"
            | "rsp"
            | "esp"
            | "sp"
            | "spl"
    ) || (8..=15).any(|index| {
        ["", "d", "w", "b"]
            .iter()
            .any(|suffix| register == format!("r{index}{suffix}"))
    })
}

fn numbered(register: &str, prefix: char, maximum: u8) -> bool {
    register
        .strip_prefix(prefix)
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| number <= maximum)
}

fn numbered_prefixed(register: &str, prefix: &str, maximum: u8) -> bool {
    register
        .strip_prefix(prefix)
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| number <= maximum)
}

fn arity(
    actual_inputs: usize,
    actual_outputs: usize,
    constrained_inputs: usize,
    constrained_outputs: usize,
) -> Result<(), String> {
    if actual_inputs != constrained_inputs {
        return Err(format!(
            "constraint list consumes {constrained_inputs} inputs, instruction provides {actual_inputs}"
        ));
    }
    if actual_outputs != constrained_outputs {
        return Err(format!(
            "constraint list has {constrained_outputs} outputs, instruction produces {actual_outputs}"
        ));
    }
    Ok(())
}

const fn arch_name(architecture: AssemblyArchitecture) -> &'static str {
    match architecture {
        AssemblyArchitecture::Aarch64 => "aarch64",
        AssemblyArchitecture::X64 => "x64",
        AssemblyArchitecture::Rv64 => "rv64",
    }
}
