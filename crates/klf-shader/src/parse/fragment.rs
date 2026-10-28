//! Single KSIR spellings read on their own: a type, a binding, an address
//! space, a target slot or a literal. The shader builder services take these
//! (section 6.6), so a definition writes `vec4<f32>` or `@builtin(position)`
//! as the text format spells it.

use super::Parser;
use super::lexer::lex;
use crate::arena::Handle;
use crate::ir::Literal;
use crate::module::{Module, TargetSlot};
use crate::types::{AddressSpace, Binding, Scalar, ScalarKind, Type};

/// Runs `read` over `text` with `module`'s types in reach, and fails unless
/// it consumes every token.
fn with_parser<T>(
    module: &mut Module,
    text: &str,
    read: impl FnOnce(&mut Parser) -> Result<T, String>,
) -> Result<T, String> {
    let lexed = lex(text)?;
    let mut parser = Parser {
        tokens: lexed.tokens,
        at: 0,
        module: std::mem::take(module),
        pending_shaders: Vec::new(),
    };
    let result = read(&mut parser).and_then(|value| match parser.peek() {
        None => Ok(value),
        Some(token) => Err(format!("unexpected {token:?} after `{text}`")),
    });
    *module = parser.module;
    result.map_err(|message| format!("`{text}`: {message}"))
}

/// A type, interned into `module`. `%Name` finds a struct already declared.
///
/// # Errors
/// Fails on text that is not one type.
pub fn ty(module: &mut Module, text: &str) -> Result<Handle<Type>, String> {
    with_parser(module, text, Parser::ty)
}

/// A binding, or none for empty text.
///
/// # Errors
/// Fails on text that is not one binding.
pub fn binding(text: &str) -> Result<Option<Binding>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    with_parser(&mut Module::default(), text, Parser::binding)
}

/// # Errors
/// Fails on text that is not one address space.
pub fn space(text: &str) -> Result<AddressSpace, String> {
    with_parser(&mut Module::default(), text, Parser::space)
}

/// # Errors
/// Fails on text that is not one target slot.
pub fn slot(text: &str) -> Result<TargetSlot, String> {
    with_parser(&mut Module::default(), text, Parser::slot)
}

/// A literal of the scalar `scalar` from source text: `true`, `false`, a
/// decimal or `0x` integer with an optional sign, or a decimal float.
///
/// # Errors
/// Fails on an unknown scalar or text that does not fit it.
pub fn literal(scalar: &str, text: &str) -> Result<Literal, String> {
    let bad = || format!("`{text}` is not a {scalar} literal");
    let scalar_type =
        Scalar::from_text(scalar).ok_or_else(|| format!("unknown scalar `{scalar}`"))?;
    let text = text.trim();
    if scalar_type.kind == ScalarKind::Bool {
        return match text {
            "true" => Ok(Literal::Bool(true)),
            "false" => Ok(Literal::Bool(false)),
            _ => Err(bad()),
        };
    }
    if scalar_type.kind == ScalarKind::Float {
        let value: f64 = text.parse().map_err(|_| bad())?;
        return Ok(match scalar_type.width {
            2 => Literal::F16(super::f32_to_half(text.parse().map_err(|_| bad())?)),
            4 => Literal::f32(text.parse().map_err(|_| bad())?),
            _ => Literal::f64(value),
        });
    }
    let (negative, digits) = text
        .strip_prefix('-')
        .map_or((false, text), |digits| (true, digits));
    let magnitude = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => i128::from_str_radix(hex, 16),
        None => digits.parse::<i128>(),
    }
    .map_err(|_| bad())?;
    let value = if negative { -magnitude } else { magnitude };
    let fits = |result: Option<Literal>| result.ok_or_else(bad);
    match (scalar_type.kind, scalar_type.width) {
        (ScalarKind::Sint, 4) => fits(i32::try_from(value).ok().map(Literal::I32)),
        (ScalarKind::Uint, 4) => fits(u32::try_from(value).ok().map(Literal::U32)),
        (ScalarKind::Sint, _) => fits(i64::try_from(value).ok().map(Literal::I64)),
        _ => fits(u64::try_from(value).ok().map(Literal::U64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_read_alone() {
        let mut module = Module::default();
        let vector = ty(&mut module, "vec4<f32>").expect("a vector");
        assert_eq!(ty(&mut module, "vec4<f32>"), Ok(vector));
        assert!(ty(&mut module, "vec4<f32> x").is_err());
        assert!(matches!(
            binding("@builtin(position)"),
            Ok(Some(Binding::BuiltIn(_)))
        ));
        assert_eq!(binding(""), Ok(None));
        assert!(matches!(
            space("storage<load store>"),
            Ok(AddressSpace::Storage { .. })
        ));
        assert_eq!(slot("universal 3"), Ok(TargetSlot::Universal(3)));
        assert_eq!(literal("u32", "0x10"), Ok(Literal::U32(16)));
        assert_eq!(literal("i32", "-4"), Ok(Literal::I32(-4)));
        assert_eq!(literal("f32", "0.5"), Ok(Literal::f32(0.5)));
        assert!(literal("u32", "-1").is_err());
    }
}
