//! The KIR object model (sections 10 and 12).

mod operation;
mod operation_rewrite;
mod operation_uses;
mod operators;
mod types;

pub use operation::{
    BlockTarget, ClosureCall, ConditionalBranch, EnumSwitch, FallibleCall, IndirectCall,
    InlineAsm, IntegerSwitch, IntrinsicCall, Operation, RawAccess, UnwindingCall, VtableCall, WitnessCall,
};
pub use operation_uses::ResultArity;
pub use operators::{
    AccessEnforcement, AccessMode, AtomicOperator, BinaryOperator, ConvertKind, DebugVariable,
    FloatBinaryOperator, FloatPredicate, FloatUnaryOperator, IntegerPredicate,
    IntegerUnaryOperator, LoadMode, MemoryOrder, Overflow, StoreMode,
};
pub use types::{Convention, FunctionType, ParameterType, Type};

use klf_core::Name;
use klf_source::Span;
use klf_target::Target;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stage {
    Raw,
    Canonical,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Module {
    pub name: Name,
    pub language: String,
    pub target: Target,
    pub stage: Stage,
    pub attributes: Vec<ModuleAttribute>,
    pub types: Vec<TypeDeclaration>,
    pub globals: Vec<GlobalDeclaration>,
    pub vtables: Vec<Vtable>,
    pub traits: Vec<TraitDeclaration>,
    pub conformances: Vec<Conformance>,
    pub imports: Vec<ImportRecord>,
    pub exports: Vec<ExportRecord>,
    pub witnesses: Vec<crate::witness::WitnessDeclaration>,
    pub functions: Vec<Function>,
}

impl Module {
    /// A module with no items, for builders that fill it in afterwards.
    #[must_use]
    pub fn empty(name: Name, language: impl Into<String>, target: Target, stage: Stage) -> Self {
        Self {
            name,
            language: language.into(),
            target,
            stage,
            attributes: Vec::new(),
            types: Vec::new(),
            globals: Vec::new(),
            vtables: Vec::new(),
            traits: Vec::new(),
            conformances: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            witnesses: Vec::new(),
            functions: Vec::new(),
        }
    }
}

/// A module-level attribute (appendix A.4 `attr`).
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModuleAttribute {
    pub name: String,
    pub value: Option<String>,
}

/// A vtable: the ordered method slots used by `call.vtable` (section 12.11).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vtable {
    pub name: Name,
    pub ty: Type,
    pub trait_name: Option<Name>,
    pub entries: Vec<Name>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraitDeclaration {
    pub name: Name,
    pub generics: Vec<Name>,
    pub associated: Vec<Name>,
    pub methods: Vec<TraitMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraitMethod {
    pub name: Name,
    pub signature: FunctionType,
}

/// A record that a type satisfies a trait, naming the witness table that
/// `call.witness` dispatches through.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conformance {
    pub ty: Type,
    pub trait_name: Name,
    pub witness: Option<Name>,
    pub conditions: Vec<(Name, Name)>,
}

/// A RIDL import record: a symbol this module uses but does not define.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ImportRecord {
    pub symbol: Name,
    pub module: String,
    pub ty: Type,
}

/// A RIDL export record: a symbol this module publishes under a RIDL name.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExportRecord {
    pub symbol: Name,
    pub ridl_name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GlobalDeclaration {
    pub name: Name,
    pub linkage: Linkage,
    pub ty: Type,
    pub init: GlobalInit,
    pub mutable: bool,
    /// Thread-local storage: each thread gets its own copy, addressed by
    /// `tls.addr` rather than `global.addr` (section 12.7).
    pub thread_local: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GlobalInit {
    Zero,
    Int(i128),
    Bytes(Vec<u8>),
}

impl GlobalInit {
    /// The leading bytes written over the global's zeroed storage; the rest
    /// stays zero. `size` is the global's byte size.
    #[must_use]
    pub fn init_bytes(&self, size: usize) -> Vec<u8> {
        match self {
            Self::Zero => Vec::new(),
            Self::Int(value) => {
                let width = size.min(8);
                value.to_le_bytes()[..width].to_vec()
            }
            Self::Bytes(bytes) => {
                let mut out = bytes.clone();
                out.truncate(size);
                out
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TypeKind {
    #[default]
    Struct,
    Enum,
    /// Fields carry explicit byte offsets and the declaration carries an
    /// explicit size and alignment (appendix A.4 `explicit`).
    Explicit,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TypeDeclaration {
    pub name: Name,
    pub generics: Vec<Name>,
    pub kind: TypeKind,
    pub layout: LayoutRule,
    pub fields: Vec<Field>,
    pub variants: Vec<EnumVariant>,
    pub open: bool,
    pub size: Option<u64>,
    pub align: Option<u32>,
    pub policy: Policy,
    pub span: Span,
}

impl TypeDeclaration {
    /// An empty non-generic struct, the shape declaration builders start from.
    #[must_use]
    pub fn empty(name: Name, layout: LayoutRule, policy: Policy, span: Span) -> Self {
        Self {
            name,
            generics: Vec::new(),
            kind: TypeKind::Struct,
            layout,
            fields: Vec::new(),
            variants: Vec::new(),
            open: false,
            size: None,
            align: None,
            policy,
            span,
        }
    }

    /// A struct declared without fields or a size: what a unit that only
    /// names a C struct knows of it. Another unit's complete declaration of
    /// the name completes it.
    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        self.kind == TypeKind::Struct
            && self.generics.is_empty()
            && self.fields.is_empty()
            && self.variants.is_empty()
            && self.size.is_none()
    }

    /// Whether two declarations describe the same type. The span records where
    /// a unit declared it, and two units that declare the same type declare it
    /// in different places, so it is not part of the comparison.
    #[must_use]
    pub fn describes_same(&self, other: &Self) -> bool {
        self.name == other.name
            && self.generics == other.generics
            && self.kind == other.kind
            && self.layout == other.layout
            && self.fields == other.fields
            && self.variants == other.variants
            && self.open == other.open
            && self.size == other.size
            && self.align == other.align
            && self.policy == other.policy
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Field {
    pub name: Name,
    pub ty: Type,
    pub offset: Option<u64>,
}

impl Field {
    /// A field whose offset the layout algorithm chooses.
    #[must_use]
    pub fn new(name: Name, ty: Type) -> Self {
        Self {
            name,
            ty,
            offset: None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnumVariant {
    pub name: Name,
    pub payload: Vec<Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayoutRule {
    Ridl,
    C,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Policy {
    pub copy: CopyPolicy,
    pub destroy: DestroyPolicy,
    pub relocate: RelocatePolicy,
    pub exclusivity: Exclusivity,
    pub pairing: Pairing,
}

impl Policy {
    /// The policy of a type the compiler may copy, move and drop by bits.
    #[must_use]
    pub fn trivial() -> Self {
        Self {
            copy: CopyPolicy::None,
            destroy: DestroyPolicy::Trivial,
            relocate: RelocatePolicy::Bitwise,
            exclusivity: Exclusivity::None,
            pairing: Pairing::None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CopyPolicy {
    Trivial,
    Implicit(Name),
    Explicit(Name),
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DestroyPolicy {
    Trivial,
    Function(Name),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RelocatePolicy {
    Bitwise,
    Function(Name),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Exclusivity {
    Static,
    Dynamic,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Pairing {
    Counted,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Linkage {
    Private,
    Module,
    Exported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Engine {
    Runtime,
    Native,
    Any,
}

/// A function attribute (section 10.3). `nounwind` is modelled separately
/// because the verifier's try-call rule (V10) reads it directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FunctionAttribute {
    Inline,
    NoInline,
    Cold,
    NoReturn,
    ReturnsTwice,
    FastMath,
    Unmanaged,
    /// Runs before the program's entry, as a C++ global's dynamic
    /// initialization does; takes no parameters and returns unit.
    Initializer,
}

impl FunctionAttribute {
    #[must_use]
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::NoInline => "noinline",
            Self::Cold => "cold",
            Self::NoReturn => "noreturn",
            Self::ReturnsTwice => "returns_twice",
            Self::FastMath => "fastmath",
            Self::Unmanaged => "unmanaged",
            Self::Initializer => "initializer",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let attribute = match text {
            "inline" => Self::Inline,
            "noinline" => Self::NoInline,
            "cold" => Self::Cold,
            "noreturn" => Self::NoReturn,
            "returns_twice" => Self::ReturnsTwice,
            "fastmath" => Self::FastMath,
            "unmanaged" => Self::Unmanaged,
            "initializer" => Self::Initializer,
            _ => return None,
        };
        Some(attribute)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Function {
    pub name: Name,
    pub linkage: Linkage,
    pub abi: FunctionAbi,
    pub nounwind: bool,
    /// A platform-C foreign import (section 26): the name is the C symbol, the
    /// body is empty, and `call.foreign` resolves its signature here.
    pub foreign: bool,
    /// What the function is at the C seam (section 26.2), when C calls it or
    /// it is a foreign import whose parameters are not plain words.
    pub c_prototype: Option<CPrototype>,
    pub generics: Vec<GenericParameter>,
    pub attributes: Vec<FunctionAttribute>,
    pub parameters: Vec<Parameter>,
    pub result: Type,
    pub failure: Option<Type>,
    pub engine: Engine,
    pub blocks: Vec<Block>,
    pub span: Span,
}

/// A function's C signature. A `Value` parameter is a C aggregate passed by
/// value, which the KIR signature carries as the address of a C-layout
/// record; a `Value` result is written through an address the KIR signature
/// takes first, and its KIR result is `unit`. Every other C type is the KIR
/// scalar of its width.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CPrototype {
    pub parameters: Vec<CType>,
    pub result: CType,
    /// For a call to a C variadic function, how many of `parameters` are its
    /// fixed ones; the rest are the variadic arguments of this call, already
    /// promoted as C promotes them.
    pub variadic: Option<usize>,
    /// For an `@FFI.Syscall` import, the `man 2` name of the Linux system
    /// call the shim makes, numbered for the target's architecture.
    pub syscall: Option<Name>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CType {
    Void,
    Bool,
    Signed(u16),
    Unsigned(u16),
    Float(u16),
    Pointer,
    Value(Name),
}

impl CPrototype {
    /// Whether only a C shim can make the crossing: an aggregate passes by
    /// value, the callee is variadic, or it is a system call.
    #[must_use]
    pub fn needs_shim(&self) -> bool {
        self.variadic.is_some()
            || self.syscall.is_some()
            || matches!(self.result, CType::Value(_))
            || self
                .parameters
                .iter()
                .any(|parameter| matches!(parameter, CType::Value(_)))
    }
}

/// The symbol of the C shim that makes `name`'s crossing, which the
/// toolchain compiles beside the program. A `#` in a KIR name, which tells
/// apart declarations of one C symbol, is spelled `__k`, which assemblers
/// read.
#[must_use]
pub fn c_shim_symbol(name: &str) -> String {
    format!("__klf_c_{}", name.replace('#', "__k"))
}

/// The C symbol a foreign function names: its KIR name up to a `#`, which
/// tells apart the declarations of one variadic C function for each shape of
/// call.
#[must_use]
pub fn c_symbol(name: &str) -> &str {
    name.split_once('#').map_or(name, |(symbol, _)| symbol)
}

#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize,
)]
pub enum FunctionAbi {
    #[default]
    Platform,
    Ridl,
}

/// A generic parameter with its trait bounds (section 10.3).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GenericParameter {
    pub name: Name,
    pub bounds: Vec<Name>,
}

impl GenericParameter {
    #[must_use]
    pub const fn unbounded(name: Name) -> Self {
        Self {
            name,
            bounds: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parameter {
    pub name: Name,
    pub convention: Option<Convention>,
    /// The `ridlcc` context parameter, passed in the context register. At most one per function, and always a pointer.
    pub context: bool,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub name: Name,
    pub parameters: Vec<Parameter>,
    pub instructions: Vec<Instruction>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Instruction {
    pub results: Vec<Name>,
    pub operation: Operation,
    pub debug_location: Option<Box<DebugLocation>>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DebugLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}
