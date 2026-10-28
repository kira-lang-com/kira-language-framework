//! Keyword tables shared by the instruction parsers.

use super::{ParseError, error};
use crate::{
    AtomicOperator, BinaryOperator, ConvertKind, FloatBinaryOperator, FloatPredicate,
    FloatUnaryOperator, IntegerPredicate, IntegerUnaryOperator, Linkage, MemoryOrder, Overflow,
};

pub(crate) fn parse_linkage(text: &str, offset: usize) -> Result<Linkage, ParseError> {
    match text {
        "private" => Ok(Linkage::Private),
        "module" => Ok(Linkage::Module),
        "exported" => Ok(Linkage::Exported),
        _ => Err(error(offset, "unknown linkage")),
    }
}

pub(crate) fn parse_overflow(text: &str, offset: usize) -> Result<Overflow, ParseError> {
    match text {
        "wrap" => Ok(Overflow::Wrap),
        "trap" => Ok(Overflow::Trap),
        "sat" => Ok(Overflow::Saturate),
        _ => Err(error(offset, "unknown overflow mode")),
    }
}

pub(crate) fn parse_predicate(text: &str, offset: usize) -> Result<IntegerPredicate, ParseError> {
    match text {
        "eq" => Ok(IntegerPredicate::Eq),
        "ne" => Ok(IntegerPredicate::Ne),
        "slt" => Ok(IntegerPredicate::SignedLt),
        "sle" => Ok(IntegerPredicate::SignedLe),
        "sgt" => Ok(IntegerPredicate::SignedGt),
        "sge" => Ok(IntegerPredicate::SignedGe),
        "ult" => Ok(IntegerPredicate::UnsignedLt),
        "ule" => Ok(IntegerPredicate::UnsignedLe),
        "ugt" => Ok(IntegerPredicate::UnsignedGt),
        "uge" => Ok(IntegerPredicate::UnsignedGe),
        _ => Err(error(offset, "unknown integer predicate")),
    }
}

pub(crate) fn parse_float_predicate(
    text: &str,
    offset: usize,
) -> Result<FloatPredicate, ParseError> {
    match text {
        "oeq" => Ok(FloatPredicate::Eq),
        "one" => Ok(FloatPredicate::Ne),
        "olt" => Ok(FloatPredicate::Lt),
        "ole" => Ok(FloatPredicate::Le),
        "ogt" => Ok(FloatPredicate::Gt),
        "oge" => Ok(FloatPredicate::Ge),
        "ord" => Ok(FloatPredicate::Ordered),
        "uno" => Ok(FloatPredicate::Unordered),
        "ueq" => Ok(FloatPredicate::UnorderedEq),
        "une" => Ok(FloatPredicate::UnorderedNe),
        "ult" => Ok(FloatPredicate::UnorderedLt),
        "ule" => Ok(FloatPredicate::UnorderedLe),
        "ugt" => Ok(FloatPredicate::UnorderedGt),
        "uge" => Ok(FloatPredicate::UnorderedGe),
        _ => Err(error(offset, "unknown float predicate")),
    }
}

pub(crate) fn parse_memory_order(text: &str, offset: usize) -> Result<MemoryOrder, ParseError> {
    match text.trim() {
        "relaxed" => Ok(MemoryOrder::Relaxed),
        "acquire" => Ok(MemoryOrder::Acquire),
        "release" => Ok(MemoryOrder::Release),
        "acq_rel" => Ok(MemoryOrder::AcquireRelease),
        "seq_cst" => Ok(MemoryOrder::SequentiallyConsistent),
        _ => Err(error(offset, "unknown memory order")),
    }
}

pub(crate) fn atomic_operators() -> [AtomicOperator; 11] {
    [
        AtomicOperator::Exchange,
        AtomicOperator::Add,
        AtomicOperator::Sub,
        AtomicOperator::And,
        AtomicOperator::Or,
        AtomicOperator::Xor,
        AtomicOperator::Nand,
        AtomicOperator::Max,
        AtomicOperator::Min,
        AtomicOperator::UnsignedMax,
        AtomicOperator::UnsignedMin,
    ]
}

pub(crate) fn binary_operators() -> [(&'static str, BinaryOperator); 15] {
    [
        ("add", BinaryOperator::Add),
        ("sub", BinaryOperator::Sub),
        ("mul", BinaryOperator::Mul),
        ("sdiv", BinaryOperator::SignedDiv),
        ("udiv", BinaryOperator::UnsignedDiv),
        ("srem", BinaryOperator::SignedRem),
        ("urem", BinaryOperator::UnsignedRem),
        ("and", BinaryOperator::And),
        ("or", BinaryOperator::Or),
        ("xor", BinaryOperator::Xor),
        ("shl", BinaryOperator::ShiftLeft),
        ("lshr", BinaryOperator::LogicalShiftRight),
        ("ashr", BinaryOperator::ArithmeticShiftRight),
        ("rotl", BinaryOperator::RotateLeft),
        ("rotr", BinaryOperator::RotateRight),
    ]
}

pub(crate) fn integer_unary_operators() -> [IntegerUnaryOperator; 7] {
    [
        IntegerUnaryOperator::Not,
        IntegerUnaryOperator::Neg,
        IntegerUnaryOperator::CountLeadingZeros,
        IntegerUnaryOperator::CountTrailingZeros,
        IntegerUnaryOperator::PopulationCount,
        IntegerUnaryOperator::ByteSwap,
        IntegerUnaryOperator::BitReverse,
    ]
}

pub(crate) fn convert_kinds() -> [ConvertKind; 12] {
    [
        ConvertKind::Truncate,
        ConvertKind::ZeroExtend,
        ConvertKind::SignExtend,
        ConvertKind::FloatTruncate,
        ConvertKind::FloatExtend,
        ConvertKind::SignedToFloat,
        ConvertKind::UnsignedToFloat,
        ConvertKind::FloatToSigned,
        ConvertKind::FloatToUnsigned,
        ConvertKind::PointerToInt,
        ConvertKind::IntToPointer,
        ConvertKind::Bitcast,
    ]
}

/// The saturating spelling of a float-to-integer conversion, for `[sat]`.
pub(crate) fn saturating(kind: ConvertKind) -> Option<ConvertKind> {
    match kind {
        ConvertKind::FloatToSigned => Some(ConvertKind::FloatToSignedSaturating),
        ConvertKind::FloatToUnsigned => Some(ConvertKind::FloatToUnsignedSaturating),
        _ => None,
    }
}

pub(crate) fn float_binary_operators() -> [(&'static str, FloatBinaryOperator); 7] {
    [
        ("fadd", FloatBinaryOperator::Add),
        ("fsub", FloatBinaryOperator::Sub),
        ("fmul", FloatBinaryOperator::Mul),
        ("fdiv", FloatBinaryOperator::Div),
        ("frem", FloatBinaryOperator::Rem),
        ("fmin", FloatBinaryOperator::Min),
        ("fmax", FloatBinaryOperator::Max),
    ]
}

pub(crate) fn float_unary_operators() -> [(&'static str, FloatUnaryOperator); 7] {
    [
        ("fneg", FloatUnaryOperator::Neg),
        ("fabs", FloatUnaryOperator::Abs),
        ("fsqrt", FloatUnaryOperator::Sqrt),
        ("ffloor", FloatUnaryOperator::Floor),
        ("fceil", FloatUnaryOperator::Ceil),
        ("ftrunc", FloatUnaryOperator::Trunc),
        ("fround", FloatUnaryOperator::Round),
    ]
}
