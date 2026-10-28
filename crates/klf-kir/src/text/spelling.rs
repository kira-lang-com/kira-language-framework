//! Textual spellings for the KIR operator and policy enumerations.

use crate::{
    BinaryOperator, CopyPolicy, DestroyPolicy, Engine, Exclusivity, FloatBinaryOperator,
    FloatPredicate, FloatUnaryOperator, IntegerPredicate, LayoutRule, Linkage, LoadMode, Overflow,
    Pairing, RelocatePolicy, StoreMode, TypeKind,
};

/// A symbol as `@` spells it: bare when every character is one a bare symbol
/// may hold, quoted otherwise.
pub(crate) struct Symbol<'a>(pub(crate) &'a str);

impl std::fmt::Display for Symbol<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bare = !self.0.is_empty()
            && self
                .0
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.#$".contains(character));
        if bare {
            formatter.write_str(self.0)
        } else {
            write!(formatter, "{:?}", self.0)
        }
    }
}

pub(crate) fn copy_policy(value: &CopyPolicy) -> String {
    match value {
        CopyPolicy::Trivial => "trivial".to_owned(),
        CopyPolicy::Implicit(function) => format!("implicit(@{})", Symbol(function.as_str())),
        CopyPolicy::Explicit(function) => format!("explicit(@{})", Symbol(function.as_str())),
        CopyPolicy::None => "none".to_owned(),
    }
}

pub(crate) fn destroy_policy(value: &DestroyPolicy) -> String {
    match value {
        DestroyPolicy::Trivial => "trivial".to_owned(),
        DestroyPolicy::Function(function) => format!("fn(@{})", Symbol(function.as_str())),
    }
}

pub(crate) fn relocate_policy(value: &RelocatePolicy) -> String {
    match value {
        RelocatePolicy::Bitwise => "bitwise".to_owned(),
        RelocatePolicy::Function(function) => format!("fn(@{})", Symbol(function.as_str())),
    }
}

pub(crate) const fn exclusivity(value: Exclusivity) -> &'static str {
    match value {
        Exclusivity::Static => "static",
        Exclusivity::Dynamic => "dynamic",
        Exclusivity::None => "none",
    }
}

pub(crate) const fn pairing(value: Pairing) -> &'static str {
    match value {
        Pairing::Counted => "counted",
        Pairing::None => "none",
    }
}

pub(crate) const fn layout_rule(value: LayoutRule) -> &'static str {
    match value {
        LayoutRule::Ridl => "ridl",
        LayoutRule::C => "c",
    }
}

pub(crate) const fn type_kind(value: TypeKind) -> &'static str {
    match value {
        TypeKind::Struct => "struct",
        TypeKind::Enum => "enum",
        TypeKind::Explicit => "explicit",
    }
}

pub(crate) const fn linkage(value: Linkage) -> &'static str {
    match value {
        Linkage::Private => "private",
        Linkage::Module => "module",
        Linkage::Exported => "exported",
    }
}

pub(crate) const fn engine(value: Engine) -> &'static str {
    match value {
        Engine::Runtime => "runtime",
        Engine::Native => "native",
        Engine::Any => "any",
    }
}

pub(crate) const fn float_binary(value: FloatBinaryOperator) -> &'static str {
    match value {
        FloatBinaryOperator::Add => "fadd",
        FloatBinaryOperator::Sub => "fsub",
        FloatBinaryOperator::Mul => "fmul",
        FloatBinaryOperator::Div => "fdiv",
        FloatBinaryOperator::Rem => "frem",
        FloatBinaryOperator::Min => "fmin",
        FloatBinaryOperator::Max => "fmax",
    }
}

pub(crate) const fn float_unary(value: FloatUnaryOperator) -> &'static str {
    match value {
        FloatUnaryOperator::Neg => "fneg",
        FloatUnaryOperator::Abs => "fabs",
        FloatUnaryOperator::Sqrt => "fsqrt",
        FloatUnaryOperator::Floor => "ffloor",
        FloatUnaryOperator::Ceil => "fceil",
        FloatUnaryOperator::Trunc => "ftrunc",
        FloatUnaryOperator::Round => "fround",
    }
}

pub(crate) const fn load_mode(value: LoadMode) -> &'static str {
    match value {
        LoadMode::Trivial => "trivial",
        LoadMode::Copy => "copy",
        LoadMode::Take => "take",
    }
}

pub(crate) const fn store_mode(value: StoreMode) -> &'static str {
    match value {
        StoreMode::Trivial => "trivial",
        StoreMode::Init => "init",
        StoreMode::Assign => "assign",
    }
}

pub(crate) const fn overflow_mode(value: Overflow) -> &'static str {
    match value {
        Overflow::Wrap => "wrap",
        Overflow::Trap => "trap",
        Overflow::Saturate => "sat",
    }
}

pub(crate) const fn binary(value: BinaryOperator) -> &'static str {
    match value {
        BinaryOperator::Add => "add",
        BinaryOperator::Sub => "sub",
        BinaryOperator::Mul => "mul",
        BinaryOperator::SignedDiv => "sdiv",
        BinaryOperator::UnsignedDiv => "udiv",
        BinaryOperator::SignedRem => "srem",
        BinaryOperator::UnsignedRem => "urem",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::Xor => "xor",
        BinaryOperator::ShiftLeft => "shl",
        BinaryOperator::LogicalShiftRight => "lshr",
        BinaryOperator::ArithmeticShiftRight => "ashr",
        BinaryOperator::RotateLeft => "rotl",
        BinaryOperator::RotateRight => "rotr",
    }
}

pub(crate) const fn predicate_name(value: IntegerPredicate) -> &'static str {
    match value {
        IntegerPredicate::Eq => "eq",
        IntegerPredicate::Ne => "ne",
        IntegerPredicate::SignedLt => "slt",
        IntegerPredicate::SignedLe => "sle",
        IntegerPredicate::SignedGt => "sgt",
        IntegerPredicate::SignedGe => "sge",
        IntegerPredicate::UnsignedLt => "ult",
        IntegerPredicate::UnsignedLe => "ule",
        IntegerPredicate::UnsignedGt => "ugt",
        IntegerPredicate::UnsignedGe => "uge",
    }
}

pub(crate) const fn float_predicate_name(value: FloatPredicate) -> &'static str {
    match value {
        FloatPredicate::Eq => "oeq",
        FloatPredicate::Ne => "one",
        FloatPredicate::Lt => "olt",
        FloatPredicate::Le => "ole",
        FloatPredicate::Gt => "ogt",
        FloatPredicate::Ge => "oge",
        FloatPredicate::Ordered => "ord",
        FloatPredicate::Unordered => "uno",
        FloatPredicate::UnorderedEq => "ueq",
        FloatPredicate::UnorderedNe => "une",
        FloatPredicate::UnorderedLt => "ult",
        FloatPredicate::UnorderedLe => "ule",
        FloatPredicate::UnorderedGt => "ugt",
        FloatPredicate::UnorderedGe => "uge",
    }
}
