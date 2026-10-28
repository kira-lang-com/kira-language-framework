//! KSIR expressions and statements (sections 11 and 12).

use crate::Span;
use crate::arena::{Handle, Range};
use crate::module::{Constant, Function, GlobalVariable, LocalVariable, Override};
use crate::types::{ScalarKind, Type};

/// A literal scalar. Floats keep their bits so equality is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Literal {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F16(u16),
    F32(u32),
    F64(u64),
}

impl Literal {
    #[must_use]
    pub fn f32(value: f32) -> Self {
        Self::F32(value.to_bits())
    }

    #[must_use]
    pub fn f64(value: f64) -> Self {
        Self::F64(value.to_bits())
    }

    #[must_use]
    pub const fn scalar(self) -> crate::types::Scalar {
        use crate::types::Scalar;
        match self {
            Self::Bool(_) => Scalar::BOOL,
            Self::I32(_) => Scalar::I32,
            Self::U32(_) => Scalar::U32,
            Self::I64(_) => Scalar {
                kind: ScalarKind::Sint,
                width: 8,
            },
            Self::U64(_) => Scalar {
                kind: ScalarKind::Uint,
                width: 8,
            },
            Self::F16(_) => Scalar::F16,
            Self::F32(_) => Scalar::F32,
            Self::F64(_) => Scalar {
                kind: ScalarKind::Float,
                width: 8,
            },
        }
    }
}

macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.name() == name)
            }
        }
    };
}

pub(crate) use named_enum;

named_enum!(UnaryOp {
    Negate => "negate",
    LogicalNot => "not",
    BitwiseNot => "bit_not",
});

named_enum!(BinaryOp {
    Add => "add",
    Subtract => "subtract",
    Multiply => "multiply",
    Divide => "divide",
    Modulo => "modulo",
    Equal => "equal",
    NotEqual => "not_equal",
    Less => "less",
    LessEqual => "less_equal",
    Greater => "greater",
    GreaterEqual => "greater_equal",
    And => "and",
    InclusiveOr => "inclusive_or",
    ExclusiveOr => "exclusive_or",
    LogicalAnd => "logical_and",
    LogicalOr => "logical_or",
    ShiftLeft => "shift_left",
    ShiftRight => "shift_right",
});

impl BinaryOp {
    #[must_use]
    pub const fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Equal
                | Self::NotEqual
                | Self::Less
                | Self::LessEqual
                | Self::Greater
                | Self::GreaterEqual
        )
    }
}

named_enum!(MathFunction {
    Abs => "abs",
    Min => "min",
    Max => "max",
    Clamp => "clamp",
    Saturate => "saturate",
    Sign => "sign",
    Sin => "sin",
    Cos => "cos",
    Tan => "tan",
    Asin => "asin",
    Acos => "acos",
    Atan => "atan",
    Atan2 => "atan2",
    Sinh => "sinh",
    Cosh => "cosh",
    Tanh => "tanh",
    Asinh => "asinh",
    Acosh => "acosh",
    Atanh => "atanh",
    Radians => "radians",
    Degrees => "degrees",
    Ceil => "ceil",
    Floor => "floor",
    Round => "round",
    Fract => "fract",
    Trunc => "trunc",
    Exp => "exp",
    Exp2 => "exp2",
    Log => "log",
    Log2 => "log2",
    Pow => "pow",
    Sqrt => "sqrt",
    InverseSqrt => "inverse_sqrt",
    Dot => "dot",
    Cross => "cross",
    Distance => "distance",
    Length => "length",
    Normalize => "normalize",
    FaceForward => "face_forward",
    Reflect => "reflect",
    Refract => "refract",
    Mix => "mix",
    Step => "step",
    SmoothStep => "smoothstep",
    Fma => "fma",
    Transpose => "transpose",
    Determinant => "determinant",
    CountOneBits => "count_one_bits",
    CountLeadingZeros => "count_leading_zeros",
    CountTrailingZeros => "count_trailing_zeros",
    FirstLeadingBit => "first_leading_bit",
    FirstTrailingBit => "first_trailing_bit",
    ReverseBits => "reverse_bits",
    ExtractBits => "extract_bits",
    InsertBits => "insert_bits",
    Pack4x8Snorm => "pack4x8snorm",
    Pack4x8Unorm => "pack4x8unorm",
    Pack2x16Snorm => "pack2x16snorm",
    Pack2x16Unorm => "pack2x16unorm",
    Pack2x16Float => "pack2x16float",
    Unpack4x8Snorm => "unpack4x8snorm",
    Unpack4x8Unorm => "unpack4x8unorm",
    Unpack2x16Snorm => "unpack2x16snorm",
    Unpack2x16Unorm => "unpack2x16unorm",
    Unpack2x16Float => "unpack2x16float",
});

impl MathFunction {
    /// How many arguments the function takes.
    #[must_use]
    pub const fn arity(self) -> usize {
        match self {
            Self::Min
            | Self::Max
            | Self::Atan2
            | Self::Pow
            | Self::Dot
            | Self::Cross
            | Self::Distance
            | Self::Reflect
            | Self::Step => 2,
            Self::Clamp
            | Self::FaceForward
            | Self::Refract
            | Self::Mix
            | Self::SmoothStep
            | Self::Fma
            | Self::ExtractBits => 3,
            Self::InsertBits => 4,
            _ => 1,
        }
    }
}

named_enum!(RelationalFunction {
    All => "all",
    Any => "any",
    IsNan => "is_nan",
    IsInf => "is_inf",
});

named_enum!(DerivativeAxis {
    X => "x",
    Y => "y",
    Width => "width",
});

named_enum!(DerivativeControl {
    Coarse => "coarse",
    Fine => "fine",
    None => "none",
});

named_enum!(AtomicFunction {
    Add => "add",
    Subtract => "subtract",
    And => "and",
    Or => "or",
    Xor => "xor",
    Min => "min",
    Max => "max",
    Exchange => "exchange",
    CompareExchange => "compare_exchange",
});

/// The mip level an `image_sample` reads (section 11.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleLevel {
    Auto,
    Zero,
    Exact(Handle<Expression>),
    Bias(Handle<Expression>),
    Gradient {
        x: Handle<Expression>,
        y: Handle<Expression>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageQuery {
    Size { level: Option<Handle<Expression>> },
    NumLevels,
    NumLayers,
    NumSamples,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    Literal(Literal),
    Constant(Handle<Constant>),
    Override(Handle<Override>),
    ZeroValue(Handle<Type>),
    Compose {
        ty: Handle<Type>,
        components: Vec<Handle<Expression>>,
    },
    Access {
        base: Handle<Expression>,
        index: Handle<Expression>,
    },
    AccessIndex {
        base: Handle<Expression>,
        index: u32,
    },
    Splat {
        size: u8,
        value: Handle<Expression>,
    },
    Swizzle {
        vector: Handle<Expression>,
        pattern: Vec<u8>,
    },
    FunctionArgument(u32),
    GlobalVariable(Handle<GlobalVariable>),
    LocalVariable(Handle<LocalVariable>),
    Load {
        pointer: Handle<Expression>,
    },
    ImageSample {
        image: Handle<Expression>,
        sampler: Handle<Expression>,
        gather: Option<u8>,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
        offset: Option<Handle<Expression>>,
        level: SampleLevel,
        depth_ref: Option<Handle<Expression>>,
    },
    ImageLoad {
        image: Handle<Expression>,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
        sample: Option<Handle<Expression>>,
        level: Option<Handle<Expression>>,
    },
    ImageQuery {
        image: Handle<Expression>,
        query: ImageQuery,
    },
    Unary {
        op: UnaryOp,
        expr: Handle<Expression>,
    },
    Binary {
        op: BinaryOp,
        left: Handle<Expression>,
        right: Handle<Expression>,
    },
    Select {
        condition: Handle<Expression>,
        accept: Handle<Expression>,
        reject: Handle<Expression>,
    },
    Derivative {
        axis: DerivativeAxis,
        control: DerivativeControl,
        expr: Handle<Expression>,
    },
    Relational {
        fun: RelationalFunction,
        argument: Handle<Expression>,
    },
    Math {
        fun: MathFunction,
        args: Vec<Handle<Expression>>,
    },
    Convert {
        expr: Handle<Expression>,
        kind: ScalarKind,
        width: Option<u8>,
    },
    Bitcast {
        expr: Handle<Expression>,
        ty: Handle<Type>,
    },
    CallResult(Handle<Function>),
    AtomicResult {
        ty: Handle<Type>,
    },
    WorkgroupUniformLoadResult {
        ty: Handle<Type>,
    },
    ArrayLength(Handle<Expression>),
}

impl Expression {
    /// Whether the expression is evaluated when an `emit` covers it
    /// (section 11.2) rather than before execution or on entry.
    #[must_use]
    pub const fn needs_emit(&self) -> bool {
        !matches!(
            self,
            Self::Literal(_)
                | Self::Constant(_)
                | Self::Override(_)
                | Self::ZeroValue(_)
                | Self::FunctionArgument(_)
                | Self::GlobalVariable(_)
                | Self::LocalVariable(_)
                | Self::CallResult(_)
                | Self::AtomicResult { .. }
                | Self::WorkgroupUniformLoadResult { .. }
        )
    }

    /// Each operand the expression reads, in order.
    #[must_use]
    pub fn operands(&self) -> Vec<Handle<Expression>> {
        let mut operands = Vec::new();
        match self {
            Self::Compose { components, .. } => operands.extend(components.iter().copied()),
            Self::Access { base, index } => operands.extend([*base, *index]),
            Self::AccessIndex { base, .. } => operands.push(*base),
            Self::Splat { value, .. } => operands.push(*value),
            Self::Swizzle { vector, .. } => operands.push(*vector),
            Self::Load { pointer } => operands.push(*pointer),
            Self::ImageSample {
                image,
                sampler,
                coordinate,
                array_index,
                offset,
                level,
                depth_ref,
                ..
            } => {
                operands.extend([*image, *sampler, *coordinate]);
                operands.extend(array_index.iter().chain(offset).copied());
                match *level {
                    SampleLevel::Exact(value) | SampleLevel::Bias(value) => operands.push(value),
                    SampleLevel::Gradient { x, y } => operands.extend([x, y]),
                    SampleLevel::Auto | SampleLevel::Zero => {}
                }
                operands.extend(depth_ref.iter().copied());
            }
            Self::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => {
                operands.extend([*image, *coordinate]);
                operands.extend(array_index.iter().chain(sample).chain(level).copied());
            }
            Self::ImageQuery { image, query } => {
                operands.push(*image);
                if let ImageQuery::Size { level: Some(level) } = query {
                    operands.push(*level);
                }
            }
            Self::Unary { expr, .. }
            | Self::Derivative { expr, .. }
            | Self::Convert { expr, .. }
            | Self::Bitcast { expr, .. } => operands.push(*expr),
            Self::Binary { left, right, .. } => operands.extend([*left, *right]),
            Self::Select {
                condition,
                accept,
                reject,
            } => operands.extend([*condition, *accept, *reject]),
            Self::Relational { argument, .. } => operands.push(*argument),
            Self::Math { args, .. } => operands.extend(args.iter().copied()),
            Self::ArrayLength(pointer) => operands.push(*pointer),
            Self::Literal(_)
            | Self::Constant(_)
            | Self::Override(_)
            | Self::ZeroValue(_)
            | Self::FunctionArgument(_)
            | Self::GlobalVariable(_)
            | Self::LocalVariable(_)
            | Self::CallResult(_)
            | Self::AtomicResult { .. }
            | Self::WorkgroupUniformLoadResult { .. } => {}
        }
        operands
    }

    /// Rewrites every operand through `map`.
    pub fn map_operands(&mut self, map: &mut impl FnMut(Handle<Expression>) -> Handle<Expression>) {
        let mut one = |handle: &mut Handle<Expression>| *handle = map(*handle);
        match self {
            Self::Compose { components, .. } => components.iter_mut().for_each(&mut one),
            Self::Access { base, index } => {
                one(base);
                one(index);
            }
            Self::AccessIndex { base, .. } => one(base),
            Self::Splat { value, .. } => one(value),
            Self::Swizzle { vector, .. } => one(vector),
            Self::Load { pointer } | Self::ArrayLength(pointer) => one(pointer),
            Self::ImageSample {
                image,
                sampler,
                coordinate,
                array_index,
                offset,
                level,
                depth_ref,
                ..
            } => {
                one(image);
                one(sampler);
                one(coordinate);
                array_index.iter_mut().for_each(&mut one);
                offset.iter_mut().for_each(&mut one);
                match level {
                    SampleLevel::Exact(value) | SampleLevel::Bias(value) => one(value),
                    SampleLevel::Gradient { x, y } => {
                        one(x);
                        one(y);
                    }
                    SampleLevel::Auto | SampleLevel::Zero => {}
                }
                depth_ref.iter_mut().for_each(&mut one);
            }
            Self::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => {
                one(image);
                one(coordinate);
                array_index.iter_mut().for_each(&mut one);
                sample.iter_mut().for_each(&mut one);
                level.iter_mut().for_each(&mut one);
            }
            Self::ImageQuery { image, query } => {
                one(image);
                if let ImageQuery::Size { level: Some(level) } = query {
                    one(level);
                }
            }
            Self::Unary { expr, .. }
            | Self::Derivative { expr, .. }
            | Self::Convert { expr, .. }
            | Self::Bitcast { expr, .. } => one(expr),
            Self::Binary { left, right, .. } => {
                one(left);
                one(right);
            }
            Self::Select {
                condition,
                accept,
                reject,
            } => {
                one(condition);
                one(accept);
                one(reject);
            }
            Self::Relational { argument, .. } => one(argument),
            Self::Math { args, .. } => args.iter_mut().for_each(&mut one),
            Self::Literal(_)
            | Self::Constant(_)
            | Self::Override(_)
            | Self::ZeroValue(_)
            | Self::FunctionArgument(_)
            | Self::GlobalVariable(_)
            | Self::LocalVariable(_)
            | Self::CallResult(_)
            | Self::AtomicResult { .. }
            | Self::WorkgroupUniformLoadResult { .. } => {}
        }
    }
}

/// The memory a barrier orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct BarrierFlags {
    pub storage: bool,
    pub workgroup: bool,
    pub image: bool,
}

impl BarrierFlags {
    #[must_use]
    pub fn words(self) -> Vec<&'static str> {
        let mut words = Vec::new();
        if self.storage {
            words.push("storage");
        }
        if self.workgroup {
            words.push("workgroup");
        }
        if self.image {
            words.push("image");
        }
        words
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwitchValue {
    I32(i32),
    U32(u32),
    Default,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    pub value: SwitchValue,
    pub body: Block,
    pub fall_through: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Emit(Range<Expression>),
    Block(Block),
    If {
        condition: Handle<Expression>,
        accept: Block,
        reject: Block,
    },
    Switch {
        selector: Handle<Expression>,
        cases: Vec<SwitchCase>,
    },
    Loop {
        body: Block,
        continuing: Block,
        break_if: Option<Handle<Expression>>,
    },
    Break,
    Continue,
    Return {
        value: Option<Handle<Expression>>,
    },
    Kill,
    Store {
        pointer: Handle<Expression>,
        value: Handle<Expression>,
    },
    ImageStore {
        image: Handle<Expression>,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
        value: Handle<Expression>,
    },
    Atomic {
        pointer: Handle<Expression>,
        fun: AtomicFunction,
        value: Handle<Expression>,
        compare: Option<Handle<Expression>>,
        result: Option<Handle<Expression>>,
    },
    Call {
        function: Handle<Function>,
        arguments: Vec<Handle<Expression>>,
        result: Option<Handle<Expression>>,
    },
    ControlBarrier(BarrierFlags),
    MemoryBarrier(BarrierFlags),
    WorkgroupUniformLoad {
        pointer: Handle<Expression>,
        result: Handle<Expression>,
    },
}

/// A sequence of statements, each with its span.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub spans: Vec<Span>,
}

impl Block {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, statement: Statement, span: Span) {
        self.statements.push(statement);
        self.spans.push(span);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.statements.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Statement, Span)> {
        self.statements.iter().zip(self.spans.iter().copied())
    }

    /// Whether the block ends in a statement control does not pass.
    #[must_use]
    pub fn terminates(&self) -> bool {
        matches!(
            self.statements.last(),
            Some(
                Statement::Return { .. } | Statement::Kill | Statement::Break | Statement::Continue
            )
        )
    }
}
