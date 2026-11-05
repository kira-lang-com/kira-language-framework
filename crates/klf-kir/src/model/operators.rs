//! The operator and mode enumerations carried by section-12 instructions.

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StoreMode {
    Trivial,
    Init,
    Assign,
}

/// The mode of `load` (section 12.8): read a trivial value, read and copy
/// through the copy policy, or move out and leave the storage uninitialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LoadMode {
    Trivial,
    Copy,
    Take,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AccessMode {
    Read,
    Modify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AccessEnforcement {
    Static,
    Dynamic,
    Unchecked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Overflow {
    Wrap,
    Trap,
    Saturate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    SignedDiv,
    UnsignedDiv,
    SignedRem,
    UnsignedRem,
    And,
    Or,
    Xor,
    ShiftLeft,
    LogicalShiftRight,
    ArithmeticShiftRight,
    RotateLeft,
    RotateRight,
}

impl BinaryOperator {
    /// Whether the operator has an `.ovf` form (section 12.2): only `add`,
    /// `sub` and `mul` do.
    #[must_use]
    pub fn has_overflow_form(self) -> bool {
        matches!(self, Self::Add | Self::Sub | Self::Mul)
    }

    /// Whether the operator accepts overflow mode `mode` (section 12.2).
    /// `add`, `sub` and `mul` take every mode. `sdiv` and `srem` also take
    /// `[trap]`, which traps on the minimum value divided by -1.
    #[must_use]
    pub fn accepts_overflow(self, mode: Overflow) -> bool {
        match mode {
            Overflow::Wrap => true,
            Overflow::Trap => {
                self.has_overflow_form() || matches!(self, Self::SignedDiv | Self::SignedRem)
            }
            Overflow::Saturate => self.has_overflow_form(),
        }
    }
}

/// The unary integer operations of section 12.2.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IntegerUnaryOperator {
    Not,
    Neg,
    CountLeadingZeros,
    CountTrailingZeros,
    PopulationCount,
    ByteSwap,
    BitReverse,
}

impl IntegerUnaryOperator {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Not => "not",
            Self::Neg => "neg",
            Self::CountLeadingZeros => "ctlz",
            Self::CountTrailingZeros => "cttz",
            Self::PopulationCount => "ctpop",
            Self::ByteSwap => "bswap",
            Self::BitReverse => "bitreverse",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum IntegerPredicate {
    Eq,
    Ne,
    SignedLt,
    SignedLe,
    SignedGt,
    SignedGe,
    UnsignedLt,
    UnsignedLe,
    UnsignedGt,
    UnsignedGe,
}

/// The `fcmp` predicates of section 12.3. The unprefixed names are the ordered
/// comparisons `oeq`, `one`, `olt`, `ole`, `ogt` and `oge`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FloatPredicate {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Ordered,
    Unordered,
    UnorderedEq,
    UnorderedNe,
    UnorderedLt,
    UnorderedLe,
    UnorderedGt,
    UnorderedGe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FloatBinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FloatUnaryOperator {
    Neg,
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConvertKind {
    Truncate,
    ZeroExtend,
    SignExtend,
    FloatTruncate,
    FloatExtend,
    SignedToFloat,
    UnsignedToFloat,
    FloatToSigned,
    FloatToUnsigned,
    /// `fptosi [sat]`: out-of-range inputs clamp instead of trapping.
    FloatToSignedSaturating,
    /// `fptoui [sat]`.
    FloatToUnsignedSaturating,
    PointerToInt,
    IntToPointer,
    Bitcast,
}

impl ConvertKind {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Truncate => "trunc",
            Self::ZeroExtend => "zext",
            Self::SignExtend => "sext",
            Self::FloatTruncate => "fptrunc",
            Self::FloatExtend => "fpext",
            Self::SignedToFloat => "sitofp",
            Self::UnsignedToFloat => "uitofp",
            Self::FloatToSigned | Self::FloatToSignedSaturating => "fptosi",
            Self::FloatToUnsigned | Self::FloatToUnsignedSaturating => "fptoui",
            Self::PointerToInt => "ptrtoint",
            Self::IntToPointer => "inttoptr",
            Self::Bitcast => "bitcast",
        }
    }

    #[must_use]
    pub fn is_saturating(self) -> bool {
        matches!(
            self,
            Self::FloatToSignedSaturating | Self::FloatToUnsignedSaturating
        )
    }
}

/// C11 memory orders, carried by the section 12.14 atomics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MemoryOrder {
    Relaxed,
    Acquire,
    Release,
    AcquireRelease,
    SequentiallyConsistent,
}

impl MemoryOrder {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Relaxed => "relaxed",
            Self::Acquire => "acquire",
            Self::Release => "release",
            Self::AcquireRelease => "acq_rel",
            Self::SequentiallyConsistent => "seq_cst",
        }
    }
}

/// The read-modify-write operations of `atomic.rmw` (section 12.14).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AtomicOperator {
    Exchange,
    Add,
    Sub,
    And,
    Or,
    Xor,
    Nand,
    Max,
    Min,
    UnsignedMax,
    UnsignedMin,
}

impl AtomicOperator {
    #[must_use]
    pub fn spelling(self) -> &'static str {
        match self {
            Self::Exchange => "xchg",
            Self::Add => "add",
            Self::Sub => "sub",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Nand => "nand",
            Self::Max => "max",
            Self::Min => "min",
            Self::UnsignedMax => "umax",
            Self::UnsignedMin => "umin",
        }
    }
}

/// The `!var(name, type, scope)` metadata of `debug.value` and `debug.addr`
/// (section 12.15).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DebugVariable {
    pub name: String,
    pub ty: crate::Type,
    pub scope: String,
}
