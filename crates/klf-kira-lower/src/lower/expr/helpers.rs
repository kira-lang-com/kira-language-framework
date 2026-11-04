use crate::ty::FeType;
use klf_kir::{FloatPredicate, IntegerPredicate};
use klf_kira_syntax::ast::{BinOp, Expr, ExprKind, Ident};

pub(super) fn comparison(op: BinOp, signed: bool) -> Option<IntegerPredicate> {
    let predicate = match op {
        BinOp::Eq => IntegerPredicate::Eq,
        BinOp::Ne => IntegerPredicate::Ne,
        BinOp::Lt => {
            if signed {
                IntegerPredicate::SignedLt
            } else {
                IntegerPredicate::UnsignedLt
            }
        }
        BinOp::Le => {
            if signed {
                IntegerPredicate::SignedLe
            } else {
                IntegerPredicate::UnsignedLe
            }
        }
        BinOp::Gt => {
            if signed {
                IntegerPredicate::SignedGt
            } else {
                IntegerPredicate::UnsignedGt
            }
        }
        BinOp::Ge => {
            if signed {
                IntegerPredicate::SignedGe
            } else {
                IntegerPredicate::UnsignedGe
            }
        }
        _ => return None,
    };
    Some(predicate)
}

pub(crate) enum Receiver<'a> {
    Expr(&'a Expr),
    Path(klf_kira_syntax::ast::Path),
}

/// If `callee` is a method call, return the expression preceding its method.
pub(crate) fn method_receiver<'a>(callee: &'a Expr, method: &str) -> Option<Receiver<'a>> {
    match &callee.kind {
        ExprKind::Field { base, name } if name.name.as_str() == method => {
            Some(Receiver::Expr(base))
        }
        ExprKind::Path(path)
            if path.segments.len() >= 2
                && path
                    .segments
                    .last()
                    .is_some_and(|s| s.name.as_str() == method) =>
        {
            let segments = path.segments[..path.segments.len() - 1].to_vec();
            Some(Receiver::Path(klf_kira_syntax::ast::Path {
                segments,
                span: path.span,
            }))
        }
        _ => None,
    }
}

/// If `callee` is `receiver.name`, return the receiver and the method name.
pub(crate) fn method_call(callee: &Expr) -> Option<(Receiver<'_>, Ident)> {
    let method = match &callee.kind {
        ExprKind::Field { name, .. } => name.clone(),
        ExprKind::Path(path) if path.segments.len() >= 2 => path.segments.last()?.clone(),
        _ => return None,
    };
    let receiver = method_receiver(callee, method.name.as_str())?;
    Some((receiver, method))
}

/// If `callee` is `receiver.append`, return the receiver as a place to lower.
pub(crate) fn append_receiver(callee: &Expr) -> Option<Receiver<'_>> {
    method_receiver(callee, "append")
}

pub(crate) fn is_string(ty: &FeType) -> bool {
    matches!(ty, FeType::Struct(name) if name.as_str() == "String")
}

/// The header struct name for a value that supports field access: a nominal
/// struct, or the `List` header behind a growable array.
pub(super) fn float_comparison(op: BinOp) -> Option<FloatPredicate> {
    let predicate = match op {
        BinOp::Eq => FloatPredicate::Eq,
        BinOp::Ne => FloatPredicate::Ne,
        BinOp::Lt => FloatPredicate::Lt,
        BinOp::Le => FloatPredicate::Le,
        BinOp::Gt => FloatPredicate::Gt,
        BinOp::Ge => FloatPredicate::Ge,
        _ => return None,
    };
    Some(predicate)
}

pub(super) fn parse_int(text: &str) -> Option<i128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, rest)
    } else {
        (10, cleaned.as_str())
    };
    i128::from_str_radix(digits, radix).ok()
}
