use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};

const VFY3: Code = Code::new(Dialect::Tir, Area::Verifier, 3);

pub(super) fn verify_instruction(instruction: &TextInstruction) -> Vec<Diagnostic> {
    if let Some(valid) = verify_trailing_modifier_groups(instruction) {
        if valid {
            return Vec::new();
        }
        return invalid_modifier_diagnostic(instruction);
    }
    let Some(modifiers) = leading_bracket_modifiers(instruction) else {
        return Vec::new();
    };
    let valid = match instruction.opcode.as_str() {
        "add" | "sub" | "mul" | "shl" => valid_overflow_mode(&modifiers),
        "add.ovf" | "sub.ovf" | "mul.ovf" | "mulh" | "urem" | "and" | "xor" | "not" | "rotl"
        | "rotr" | "fshl" | "fshr" | "smin" | "smax" | "umin" | "umax" | "icmp" | "select"
        | "ctpop" | "bswap" | "bitreverse" | "clmul" | "clmulh" | "freeze" => modifiers.is_empty(),
        "sdiv" => valid_optional_mode(&modifiers, &["trap", "wrap"], true),
        "udiv" => only_flags(&modifiers, &["exact"]),
        "srem" => valid_optional_mode(&modifiers, &["trap", "wrap"], false),
        "lshr" | "ashr" => valid_shift_mode(&modifiers),
        "neg" | "abs" => valid_optional_mode(&modifiers, &["wrap", "trap", "poison"], false),
        "or" => only_flags(&modifiers, &["disjoint"]),
        "ctlz" | "cttz" => only_flags(&modifiers, &["poison"]),
        "fadd" | "fsub" | "fmul" | "fdiv" | "frem" | "fneg" | "fabs" | "fsqrt" | "fma"
        | "fmuladd" | "fmin" | "fmax" | "fminnum" | "fmaxnum" | "fcopysign" | "ffloor"
        | "fceil" | "ftrunc" | "fround" | "froundeven" | "fcmp" | "fclass" | "frexp" | "ldexp" => {
            valid_float_modifiers(&modifiers, &instruction.opcode)
        }
        opcode if opcode.starts_with("math.") => valid_float_modifiers(&modifiers, opcode),
        "fp.flags.read" | "fp.flags.clear" => modifiers.is_empty(),
        "fptrunc" | "sitofp" | "uitofp" => valid_rounding_modifiers(&modifiers),
        "fptosi" | "fptoui" => valid_float_to_integer_modifiers(&modifiers),
        "fcvt" => valid_fcvt_modifiers(&modifiers),
        "load" => valid_optional_selector(&modifiers, &["trivial", "copy", "take"]),
        "store" => valid_optional_selector(&modifiers, &["trivial", "init", "assign"]),
        "copy.addr" | "move.addr" => valid_optional_selector(&modifiers, &["init", "assign"]),
        "load.borrow" | "destroy.addr" | "ptr.diff" => modifiers.is_empty(),
        "raw.load" | "raw.store" => valid_raw_memory_modifiers(&modifiers),
        "mem.copy" => valid_flags(&modifiers, &["volatile", "nooverlap"]),
        "mem.move" | "mem.set" => valid_flags(&modifiers, &["volatile"]),
        "ptr.offset" => valid_flags(&modifiers, &["inbounds"]),
        "elem.addr" => valid_flags(&modifiers, &["unchecked"]),
        "heap.alloc.raw" => valid_flags(&modifiers, &["uninit"]),
        "prefetch" => valid_prefetch_modifiers(&modifiers),
        "vec.reduce" => only_flags(&modifiers, &["reassoc"]),
        "vec.splat" | "vec.extract" | "vec.insert" | "vec.shuffle" | "vec.step" | "vec.vscale"
        | "vec.load" | "vec.store" | "vec.gather" | "vec.scatter" | "vec.compress"
        | "vec.expand" | "vec.dot" | "vec.zip" | "vec.unzip" | "vec.reverse" | "vec.slide"
        | "vec.concat" | "vec.split" | "vec.mask.active" | "vec.mask.count" | "vec.mask.first"
        | "vec.table" => modifiers.is_empty(),
        "mmio.load" => valid_order_modifier(&modifiers, &["relaxed", "acquire"]),
        "mmio.store" => valid_order_modifier(&modifiers, &["relaxed", "release"]),
        "fence.io" => valid_optional_selector(&modifiers, &["read", "write", "full"]),
        "stack.alloc" => valid_stack_alloc_modifiers(&modifiers),
        "access.begin" => valid_access_begin_modifiers(&modifiers),
        "unreachable" => valid_optional_selector(&modifiers, &["trap", "ub"]),
        _ => true,
    };
    if valid {
        Vec::new()
    } else {
        invalid_modifier_diagnostic(instruction)
    }
}

fn verify_trailing_modifier_groups(instruction: &TextInstruction) -> Option<bool> {
    let opcode = instruction.opcode.as_str();
    let (start, validate): (usize, fn(&[Vec<String>]) -> bool) = match opcode {
        "raw.load" | "raw.store" => (
            first_top_level_comma(&instruction.tokens)? + 1,
            valid_raw_memory_modifiers,
        ),
        "prefetch" => (
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
            valid_prefetch_modifiers,
        ),
        "mem.copy" => (
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
            |groups| valid_flags(groups, &["volatile", "nooverlap"]),
        ),
        "mem.move" | "mem.set" => (
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
            |groups| valid_flags(groups, &["volatile"]),
        ),
        "copy.addr" | "move.addr" => {
            let to = instruction
                .tokens
                .iter()
                .position(|token| matches!(&token.kind, TokenKind::Atom(value) if value == "to"))?;
            (to + 1, |groups| {
                valid_optional_selector(groups, &["init", "assign"])
            })
        }
        "heap.alloc.raw" => (
            crate::ssa::text_instruction_operands_start(&instruction.tokens),
            |groups| valid_flags(groups, &["uninit"]),
        ),
        _ => return None,
    };
    Some(
        trailing_bracket_groups(&instruction.tokens, start).is_some_and(|groups| validate(&groups)),
    )
}

fn invalid_modifier_diagnostic(instruction: &TextInstruction) -> Vec<Diagnostic> {
    vec![
        Diagnostic::error(
            VFY3,
            format!("{} has invalid or repeated modifiers", instruction.opcode),
        )
        .with_primary_span(instruction.span),
    ]
}

fn first_top_level_comma(tokens: &[Token]) -> Option<usize> {
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let mut nesting = 0usize;
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| {
            match token.kind {
                TokenKind::Punctuation('<' | '(' | '[' | '{') => nesting += 1,
                TokenKind::Punctuation('>' | ')' | ']' | '}') => {
                    nesting = nesting.saturating_sub(1)
                }
                TokenKind::Punctuation(',') if nesting == 0 => return Some(index),
                _ => {}
            }
            None
        })
}

fn trailing_bracket_groups(tokens: &[Token], start: usize) -> Option<Vec<Vec<String>>> {
    let mut groups = Vec::new();
    let mut cursor = start;
    while cursor < tokens.len() {
        if !matches!(tokens[cursor].kind, TokenKind::Punctuation('[')) {
            cursor += 1;
            continue;
        }
        cursor += 1;
        let mut group = Vec::new();
        while cursor < tokens.len() && !matches!(tokens[cursor].kind, TokenKind::Punctuation(']')) {
            match &tokens[cursor].kind {
                TokenKind::Atom(atom) => group.push(atom.clone()),
                TokenKind::Punctuation('=') => group.push("=".to_owned()),
                TokenKind::Punctuation('%') => {
                    let Token {
                        kind: TokenKind::Atom(name),
                        ..
                    } = tokens.get(cursor + 1)?
                    else {
                        return None;
                    };
                    group.push(format!("%{name}"));
                    cursor += 1;
                }
                TokenKind::Punctuation(',') => {}
                _ => return None,
            }
            cursor += 1;
        }
        if cursor == tokens.len() {
            return None;
        }
        groups.push(group);
        cursor += 1;
    }
    Some(groups)
}

pub(super) fn leading_bracket_modifiers(instruction: &TextInstruction) -> Option<Vec<Vec<String>>> {
    let tokens = &instruction.tokens;
    let mut cursor = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        loop {
            cursor += 2;
            match tokens.get(cursor).map(|token| &token.kind) {
                Some(TokenKind::Punctuation('=')) => {
                    cursor += 1;
                    break;
                }
                Some(TokenKind::Punctuation(',')) => cursor += 1,
                _ => return None,
            }
        }
    }
    cursor += 1;
    while cursor + 1 < tokens.len() && matches!(tokens[cursor].kind, TokenKind::Punctuation('.')) {
        cursor += 2;
    }

    let mut groups = Vec::new();
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('['))
    ) {
        let open = cursor;
        cursor += 1;
        let mut group = Vec::new();
        while cursor < tokens.len() && !matches!(tokens[cursor].kind, TokenKind::Punctuation(']')) {
            match &tokens[cursor].kind {
                TokenKind::Atom(atom) => group.push(atom.clone()),
                TokenKind::Punctuation('=') => group.push("=".to_owned()),
                TokenKind::Punctuation('%') => {
                    let Some(Token {
                        kind: TokenKind::Atom(name),
                        ..
                    }) = tokens.get(cursor + 1)
                    else {
                        return Some(vec![vec![format!("<malformed:{}>", open)]]);
                    };
                    group.push(format!("%{name}"));
                    cursor += 1;
                }
                TokenKind::Punctuation(',') => {}
                _ => return Some(vec![vec![format!("<malformed:{}>", open)]]),
            }
            cursor += 1;
        }
        if cursor == tokens.len() {
            return Some(vec![vec!["<unterminated>".to_owned()]]);
        }
        groups.push(group);
        cursor += 1;
    }
    Some(groups)
}

fn only_flags(groups: &[Vec<String>], allowed: &[&str]) -> bool {
    let mut seen = Vec::new();
    for group in groups {
        if group.is_empty() {
            return false;
        }
        for modifier in group {
            if modifier == "=" || !allowed.contains(&modifier.as_str()) || seen.contains(modifier) {
                return false;
            }
            seen.push(modifier.clone());
        }
    }
    true
}

fn valid_flags(groups: &[Vec<String>], allowed: &[&str]) -> bool {
    groups
        .iter()
        .all(|group| group.len() == 1 && allowed.contains(&group[0].as_str()))
        && groups
            .iter()
            .flatten()
            .collect::<std::collections::HashSet<_>>()
            .len()
            == groups.len()
}

fn valid_optional_selector(groups: &[Vec<String>], alternatives: &[&str]) -> bool {
    groups.len() <= 1
        && groups
            .first()
            .is_none_or(|group| group.len() == 1 && alternatives.contains(&group[0].as_str()))
}

fn valid_raw_memory_modifiers(groups: &[Vec<String>]) -> bool {
    let mut alignment_seen = false;
    let mut flags = Vec::new();
    for group in groups {
        match group.as_slice() {
            [name, value] if name == "align" && !alignment_seen => {
                let Some(alignment) = tir_format::numeric_literals::parse_unsigned_integer(value)
                else {
                    return false;
                };
                if alignment == 0 || alignment > u128::from(u64::MAX) {
                    return false;
                }
                alignment_seen = true;
            }
            [flag] if ["volatile", "nontemporal"].contains(&flag.as_str()) => {
                if flags.contains(&flag.as_str()) {
                    return false;
                }
                flags.push(flag.as_str());
            }
            _ => return false,
        }
    }
    true
}

fn valid_prefetch_modifiers(groups: &[Vec<String>]) -> bool {
    let mut access_seen = false;
    let mut locality_seen = false;
    let mut kind_seen = false;
    for group in groups {
        match group.as_slice() {
            [access] if ["read", "write"].contains(&access.as_str()) && !access_seen => {
                access_seen = true;
            }
            [name, value] if name == "locality" && !locality_seen => {
                let Some(locality) = tir_format::numeric_literals::parse_unsigned_integer(value)
                else {
                    return false;
                };
                if locality > 3 {
                    return false;
                }
                locality_seen = true;
            }
            [kind] if ["data", "instruction"].contains(&kind.as_str()) && !kind_seen => {
                kind_seen = true;
            }
            _ => return false,
        }
    }
    true
}

fn valid_order_modifier(groups: &[Vec<String>], orders: &[&str]) -> bool {
    groups.len() <= 1
        && groups.first().is_none_or(|group| {
            group.len() == 2 && group[0] == "order" && orders.contains(&group[1].as_str())
        })
}

fn valid_stack_alloc_modifiers(groups: &[Vec<String>]) -> bool {
    groups.len() <= 1
        && groups.first().is_none_or(|group| {
            group.len() == 2 && group[0] == "count" && group[1].starts_with('%')
        })
}

fn valid_access_begin_modifiers(groups: &[Vec<String>]) -> bool {
    let mut access_seen = false;
    let mut mode_seen = false;
    for group in groups {
        if group.len() != 1 {
            return false;
        }
        let value = group[0].as_str();
        if ["read", "modify"].contains(&value) && !access_seen {
            access_seen = true;
        } else if ["static", "dynamic", "unchecked"].contains(&value) && !mode_seen {
            mode_seen = true;
        } else {
            return false;
        }
    }
    access_seen && mode_seen
}

fn valid_overflow_mode(groups: &[Vec<String>]) -> bool {
    let mut modes = 0;
    for group in groups {
        if group == &["wrap"] {
            modes += 1;
        } else if group.len() == 2
            && ["trap", "sat", "poison"].contains(&group[0].as_str())
            && ["s", "u"].contains(&group[1].as_str())
        {
            modes += 1;
        } else {
            return false;
        }
    }
    modes <= 1
}

fn valid_optional_mode(groups: &[Vec<String>], modes: &[&str], allow_exact: bool) -> bool {
    let mut mode_seen = false;
    let mut exact_seen = false;
    for group in groups {
        if group.len() == 1 && modes.contains(&group[0].as_str()) && !mode_seen {
            mode_seen = true;
        } else if allow_exact && group.as_slice() == ["exact"] && !exact_seen {
            exact_seen = true;
        } else {
            return false;
        }
    }
    true
}

fn valid_shift_mode(groups: &[Vec<String>]) -> bool {
    valid_optional_mode(groups, &["wrap", "trap"], true)
}

fn valid_rounding_modifiers(groups: &[Vec<String>]) -> bool {
    let mut rounding_seen = false;
    groups.iter().all(|group| {
        if group.len() == 3 && group[0] == "rm" && group[1] == "=" && !rounding_seen {
            rounding_seen = true;
            ["rne", "rtz", "rup", "rdn", "rna"].contains(&group[2].as_str())
        } else {
            false
        }
    })
}

fn valid_float_to_integer_modifiers(groups: &[Vec<String>]) -> bool {
    groups.len() == 1
        && groups[0].len() == 1
        && ["sat", "trap", "poison"].contains(&groups[0][0].as_str())
}

fn valid_fcvt_modifiers(groups: &[Vec<String>]) -> bool {
    let mut rounding_seen = false;
    let mut mode_seen = false;
    for group in groups {
        if group.len() == 3 && group[0] == "rm" && group[1] == "=" && !rounding_seen {
            rounding_seen = true;
            if !["rne", "rtz", "rup", "rdn", "rna"].contains(&group[2].as_str()) {
                return false;
            }
        } else if group.len() == 1
            && ["sat", "satfinite", "nonsat"].contains(&group[0].as_str())
            && !mode_seen
        {
            mode_seen = true;
        } else {
            return false;
        }
    }
    true
}

fn valid_float_modifiers(groups: &[Vec<String>], opcode: &str) -> bool {
    let mut seen = Vec::new();
    for group in groups {
        if group.is_empty() {
            return false;
        }
        if group.len() == 3 && group[0] == "rm" && group[1] == "=" {
            if !rounding_is_meaningful(opcode)
                || seen.contains(&"rm")
                || !["rne", "rtz", "rup", "rdn", "rna"].contains(&group[2].as_str())
            {
                return false;
            }
            seen.push("rm");
            continue;
        }
        for modifier in group {
            let supported = matches!(
                modifier.as_str(),
                "nnan"
                    | "ninf"
                    | "nsz"
                    | "arcp"
                    | "contract"
                    | "afn"
                    | "reassoc"
                    | "fast"
                    | "strict"
                    | "approx"
            );
            if !supported
                || (modifier == "approx" && !opcode.starts_with("math."))
                || seen.contains(&modifier.as_str())
            {
                return false;
            }
            seen.push(modifier.as_str());
        }
    }
    true
}

fn rounding_is_meaningful(opcode: &str) -> bool {
    matches!(
        opcode,
        "fadd" | "fsub" | "fmul" | "fdiv" | "frem" | "fsqrt" | "fma" | "fmuladd" | "ldexp"
    ) || opcode.starts_with("math.")
}
