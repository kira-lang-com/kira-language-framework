//! KSIR, the KLF Shader IR (`KLF-KSL.md` sections 7 to 15 and 18): the
//! data model, layout rules, the builder definitions lower through, type
//! resolution, and the text form.
//!
//! KSIR arithmetic is bit-exact over fixed-width integers and floats, so the
//! narrowing and sign casts are deliberate; the printers, parser and type
//! resolver dispatch over every expression kind in one match each.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::float_cmp
)]

pub mod arena;
pub mod builder;
pub use check::check_expression;
mod check;
pub mod ir;
pub mod layout;
pub mod module;
pub mod parse;
pub mod print;
pub mod resolve;
pub mod target;
pub mod types;
pub mod visit;

pub use arena::{Arena, Handle, Range, UniqueArena};
pub use ir::{
    AtomicFunction, BarrierFlags, BinaryOp, Block, DerivativeAxis, DerivativeControl, Expression,
    ImageQuery, Literal, MathFunction, RelationalFunction, SampleLevel, Statement, SwitchCase,
    SwitchValue, UnaryOp,
};
pub use module::{
    Constant, EarlyDepthTest, EntryPoint, Function, FunctionArgument, FunctionRef, FunctionResult,
    GlobalVariable, GroupClass, GroupDecl, HlslClass, LocalVariable, Module, Override,
    ResourceBinding, ShaderDecl, ShaderKind, Stage, TargetSlot, WorkgroupExtent,
};
pub use target::Target;
pub use types::{
    Access, AddressSpace, ArraySize, Binding, BuiltIn, Capability, ImageClass, ImageDim,
    Interpolation, LayoutRule, Sampling, Scalar, ScalarKind, StorageFormat, StructMember, Type,
    TypeInner,
};

/// Where a node came from: a file of [`Module::files`] and a 1-based line
/// and column, plus the byte range when the producer knew it. Line 0 means
/// no position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: u32,
    pub line: u32,
    pub column: u32,
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn is_known(self) -> bool {
        self.line != 0
    }
}
