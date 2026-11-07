//! Restricted scalar floating-point operation encoding (TIR 12.3).
//! Rounding modes and fast-math modifiers remain unsupported by this text path.
use tir_diag::Span;

use crate::text::TokenKind;
use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) struct ParsedFloatOperation {
    pub(crate) opcode: u64,
    pub(crate) result: String,
    pub(crate) operands: Vec<String>,
    pub(crate) modifiers: u64,
    pub(crate) is_comparison: bool,
}

const PREDICATES: [&str; 16] = [
    "oeq", "one", "olt", "ole", "ogt", "oge", "ord", "uno", "ueq", "une", "ult", "ule", "ugt",
    "uge", "true", "false",
];

pub(crate) fn predicate_name(selector: u64) -> Option<&'static str> {
    usize::try_from(selector)
        .ok()
        .and_then(|index| PREDICATES.get(index))
        .copied()
}

pub(crate) fn operation_info(name: &str) -> Option<(u64, usize)> {
    Some(match name {
        "fadd" => (0x060, 2),
        "fsub" => (0x061, 2),
        "fmul" => (0x062, 2),
        "fdiv" => (0x063, 2),
        "frem" => (0x064, 2),
        "fneg" => (0x065, 1),
        "fabs" => (0x066, 1),
        "fsqrt" => (0x067, 1),
        "fma" => (0x068, 3),
        "fmuladd" => (0x069, 3),
        "fmin" => (0x06a, 2),
        "fmax" => (0x06b, 2),
        "fminnum" => (0x06c, 2),
        "fmaxnum" => (0x06d, 2),
        "fcopysign" => (0x06e, 2),
        "ffloor" => (0x06f, 1),
        "fceil" => (0x070, 1),
        "ftrunc" => (0x071, 1),
        "fround" => (0x072, 1),
        "froundeven" => (0x073, 1),
        "fcmp" => (0x074, 2),
        "math.sin" => (0x07a, 1),
        "math.cos" => (0x07b, 1),
        "math.tan" => (0x07c, 1),
        "math.asin" => (0x07d, 1),
        "math.acos" => (0x07e, 1),
        "math.atan" => (0x07f, 1),
        "math.atan2" => (0x080, 2),
        "math.sinh" => (0x081, 1),
        "math.cosh" => (0x082, 1),
        "math.tanh" => (0x083, 1),
        "math.asinh" => (0x084, 1),
        "math.acosh" => (0x085, 1),
        "math.atanh" => (0x086, 1),
        "math.exp" => (0x087, 1),
        "math.exp2" => (0x088, 1),
        "math.exp10" => (0x089, 1),
        "math.expm1" => (0x08a, 1),
        "math.log" => (0x08b, 1),
        "math.log2" => (0x08c, 1),
        "math.log10" => (0x08d, 1),
        "math.log1p" => (0x08e, 1),
        "math.pow" => (0x08f, 2),
        "math.cbrt" => (0x090, 1),
        "math.hypot" => (0x091, 2),
        "math.rsqrt" => (0x092, 1),
        _ => return None,
    })
}

pub(crate) fn operation_name(opcode: u64) -> Option<(&'static str, usize)> {
    let name = match opcode {
        0x060 => "fadd",
        0x061 => "fsub",
        0x062 => "fmul",
        0x063 => "fdiv",
        0x064 => "frem",
        0x065 => "fneg",
        0x066 => "fabs",
        0x067 => "fsqrt",
        0x068 => "fma",
        0x069 => "fmuladd",
        0x06a => "fmin",
        0x06b => "fmax",
        0x06c => "fminnum",
        0x06d => "fmaxnum",
        0x06e => "fcopysign",
        0x06f => "ffloor",
        0x070 => "fceil",
        0x071 => "ftrunc",
        0x072 => "fround",
        0x073 => "froundeven",
        0x074 => "fcmp",
        0x07a => "math.sin",
        0x07b => "math.cos",
        0x07c => "math.tan",
        0x07d => "math.asin",
        0x07e => "math.acos",
        0x07f => "math.atan",
        0x080 => "math.atan2",
        0x081 => "math.sinh",
        0x082 => "math.cosh",
        0x083 => "math.tanh",
        0x084 => "math.asinh",
        0x085 => "math.acosh",
        0x086 => "math.atanh",
        0x087 => "math.exp",
        0x088 => "math.exp2",
        0x089 => "math.exp10",
        0x08a => "math.expm1",
        0x08b => "math.log",
        0x08c => "math.log2",
        0x08d => "math.log10",
        0x08e => "math.log1p",
        0x08f => "math.pow",
        0x090 => "math.cbrt",
        0x091 => "math.hypot",
        0x092 => "math.rsqrt",
        _ => return None,
    };
    operation_info(name).map(|(_, count)| (name, count))
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<ParsedFloatOperation, TextParseError> {
    let Some((opcode, count)) = operation_info(&instruction.opcode) else {
        return Err(error(
            item,
            instruction.span,
            "unsupported floating-point opcode",
        ));
    };
    let tokens = &instruction.tokens;
    let [percent, name, equal, mnemonic, ..] = tokens.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "floating-point operation is incomplete",
        ));
    };
    let (
        TokenKind::Punctuation('%'),
        TokenKind::Atom(result),
        TokenKind::Punctuation('='),
        TokenKind::Atom(mnemonic),
    ) = (&percent.kind, &name.kind, &equal.kind, &mnemonic.kind)
    else {
        return Err(error(
            item,
            instruction.span,
            "expected `%result = float-op ...`",
        ));
    };
    if result != &instruction.results.first().cloned().unwrap_or_default()
        || instruction.results.len() != 1
    {
        return Err(error(
            item,
            instruction.span,
            "floating-point SSA result is malformed",
        ));
    }
    let mut start = 4;
    let actual_mnemonic = if instruction.opcode.starts_with("math.") {
        let Some([dot, suffix]) = tokens.get(4..6) else {
            return Err(error(
                item,
                instruction.span,
                "math operation suffix is missing",
            ));
        };
        if !matches!(dot.kind, TokenKind::Punctuation('.')) {
            return Err(error(
                item,
                instruction.span,
                "invalid math operation separator",
            ));
        }
        let TokenKind::Atom(suffix) = &suffix.kind else {
            return Err(error(
                item,
                instruction.span,
                "invalid math operation suffix",
            ));
        };
        start = 6;
        format!("{mnemonic}.{suffix}")
    } else {
        mnemonic.clone()
    };
    if actual_mnemonic != instruction.opcode {
        return Err(error(
            item,
            instruction.span,
            "floating-point opcode spelling does not match",
        ));
    }
    let mut modifiers = 0;
    if instruction.opcode.starts_with("math.")
        && tokens.get(start..start + 3).is_some_and(|slice| {
            matches!(slice,
            [open, mode, close] if matches!(open.kind, TokenKind::Punctuation('['))
                && matches!(&mode.kind, TokenKind::Atom(value) if value == "approx")
                && matches!(close.kind, TokenKind::Punctuation(']')))
        })
    {
        modifiers |= 1 << 17;
        start += 3;
    }
    let is_comparison = opcode == 0x074;
    if is_comparison {
        let Some(TokenKind::Atom(predicate)) = tokens.get(start).map(|token| &token.kind) else {
            return Err(error(item, instruction.span, "fcmp requires a predicate"));
        };
        let Some(selector) = PREDICATES
            .iter()
            .position(|candidate| candidate == predicate)
        else {
            return Err(error(item, instruction.span, "unknown fcmp predicate"));
        };
        modifiers = (selector as u64) << 48;
        start += 1;
    }
    let expected_length = start + count * 2 + count.saturating_sub(1);
    if tokens.len() != expected_length {
        return Err(error(
            item,
            instruction.span,
            "floating-point operand count is invalid",
        ));
    }
    let mut operands = Vec::with_capacity(count);
    for operand_index in 0..count {
        let offset = start + operand_index * 3;
        if operand_index != 0 && !matches!(tokens[offset - 1].kind, TokenKind::Punctuation(',')) {
            return Err(error(
                item,
                instruction.span,
                "floating-point operands must be comma-separated",
            ));
        }
        let [percent, name] = &tokens[offset..offset + 2] else {
            unreachable!()
        };
        let (TokenKind::Punctuation('%'), TokenKind::Atom(name)) = (&percent.kind, &name.kind)
        else {
            return Err(error(
                item,
                instruction.span,
                "floating-point operands must be SSA values",
            ));
        };
        operands.push(name.clone());
    }
    Ok(ParsedFloatOperation {
        opcode,
        result: result.clone(),
        operands,
        modifiers,
        is_comparison,
    })
}

#[cfg(test)]
mod math_tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    #[test]
    fn math_scalar_unary_binary_and_approx_round_trip() {
        let source = "module @Math tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Math.run(%a: f64, %b: f64) -> f64 engine(native) {\nbb0(%a: f64, %b: f64):\n  %sin = math.sin %a\n  %cos = math.cos [approx] %b\n  %sum = math.hypot %sin, %cos\n  %power = math.pow %sum, %b\n  %result = math.rsqrt %power\n  return %result\n}\n";
        let parsed = parse_text_module(source).expect("math text parses");
        let bytes = assemble_text_module(&parsed).expect("math CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("math binary decodes");
        let text = disassemble_text_module(&binary, u64::MAX).expect("math disassembles");
        assert!(text.contains("math.cos [approx]"));
        assert_eq!(
            bytes,
            assemble_text_module(&parse_text_module(&text).expect("canonical math parses"))
                .expect("math reassembles")
        );
    }
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

    fn module(body: &str) -> String {
        format!(
            "module @Float tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Float.run(%a: f32, %b: f32) -> f32 engine(native) {{\nbb0(%a: f32, %b: f32):\n{body}\n}}\n"
        )
    }

    fn round_trip(body: &str) {
        let source = module(body);
        let parsed = parse_text_module(&source).expect("float text parses");
        let bytes = assemble_text_module(&parsed).expect("float chain assembles");
        let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
        let canonical =
            disassemble_text_module(&decoded, u64::MAX).expect("float chain disassembles");
        let rebuilt =
            assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
                .expect("canonical float chain reassembles");
        assert_eq!(bytes, rebuilt);
    }

    #[test]
    fn float_arithmetic_fma_and_unary_chain_round_trip() {
        round_trip(
            "  %sum = fadd %a, %b\n  %product = fmul %sum, %b\n  %fused = fma %product, %a, %b\n  %neg = fneg %fused\n  %result = fsqrt %neg\n  return %result",
        );
    }

    #[test]
    fn float_comparison_then_select_round_trip() {
        for predicate in ["oeq", "olt", "uge", "ord", "uno", "true", "false"] {
            round_trip(&format!(
                "  %less = fcmp {predicate} %a, %b\n  %sum = fsub %a, %b\n  %result = select %less, %sum, %a\n  return %result"
            ));
        }
    }

    #[test]
    fn malformed_float_operands_are_rejected() {
        for body in [
            "  %wrong = fadd %a, %undefined\n  return %wrong",
            "  %wrong = fadd %a\n  return %wrong",
            "  %wrong = fma %a, %b\n  return %wrong",
            "  %wrong = fcmp bad %a, %b\n  return %a",
            "  %wrong = fdiv %a, 7\n  return %wrong",
        ] {
            let parsed = parse_text_module(&module(body)).expect("text syntax parses");
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad float CODE assembled: {body}"
            );
        }
    }
}

#[cfg(test)]
mod vector_tests {
    use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

    #[test]
    fn float_vector_arithmetic_and_mask_select_round_trip() {
        for lanes in [2, 3, 4, 8] {
            let ty = format!("vec<{lanes} x f32>");
            let source = format!(
                "module @Float tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Float.vectors(%a: {ty}, %b: {ty}) -> {ty} engine(native) {{\nbb0(%a: {ty}, %b: {ty}):\n  %sum = fadd %a, %b\n  %difference = fsub %sum, %b\n  %mask = fcmp olt %sum, %b\n  %result = select %mask, %difference, %a\n  return %result\n}}\n"
            );
            let parsed = parse_text_module(&source).expect("vector text parses");
            let bytes =
                assemble_text_module(&parsed).expect("vector floating-point chain assembles");
            let binary = BinaryModule::decode(&bytes).expect("vector CODE decodes");
            let canonical = disassemble_text_module(&binary, u64::MAX)
                .expect("vector float comparison disassembles");
            assert!(canonical.contains(&format!("vec<{lanes} x f32>")));
            assert!(canonical.contains("fcmp olt"));
            let rebuilt =
                assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
                    .expect("canonical vector CODE reassembles");
            assert_eq!(bytes, rebuilt);
        }
    }

    #[test]
    fn vector_float_select_rejects_wrong_mask_lane_count() {
        let source = "module @Float tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Float.invalid(%a: vec<4 x f32>, %mask: vec<2 x i1>) -> vec<4 x f32> engine(native) {\nbb0(%a: vec<4 x f32>, %mask: vec<2 x i1>):\n  %result = select %mask, %a, %a\n  return %result\n}\n";
        let parsed = parse_text_module(source).expect("text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "wrong mask width must fail"
        );
    }
}
