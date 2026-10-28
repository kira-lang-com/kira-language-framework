//! Compiled bytecode program: functions, code words and the constant pool.

use crate::bytecode::WidthCode;
use klf_core::Name;

/// A constant pooled in the `CONSTS` section and loaded with `LOADK`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Const {
    Int(i128),
    Float(u64),
    String(String),
}

/// A resolved call target for the call-site table (section 15.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallSite {
    pub callee: usize,
    pub first_arg: u16,
    pub arg_count: u16,
    pub result: u16,
    pub has_result: bool,
    /// A `call.foreign` (section 15.6): the index of the import it calls, or
    /// whose address it takes. `None` is an ordinary call to `callee`.
    pub foreign: Option<u32>,
    /// A `call.indirect`: the register holding the target function's index,
    /// resolved at run time. `None` is a direct call to `callee`.
    pub target: Option<u16>,
    /// A `try.call`: where its normal and error targets are, and the register
    /// that says which one the call took. `None` is a call whose failure
    /// propagates out of the caller.
    pub fallible: Option<FallibleSite>,
}

/// The targets of a `try.call` (section 15.5). The call writes the callee's
/// result, or on failure its error, to the site's result register, and sets
/// `failed` to 1 on failure and 0 otherwise. `branch` is the branch site that
/// reads `failed`: its then-edge is the error target and its else-edge the
/// normal target, each receiving the result register as the leading block
/// parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FallibleSite {
    pub failed: u16,
    pub branch: u32,
}

/// A static allocation shape: byte size and alignment of a stack or heap slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllocShape {
    pub size: u32,
    pub align: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterMove {
    pub source: u16,
    pub destination: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchTarget {
    pub target: usize,
    pub moves: Vec<RegisterMove>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchCondition {
    Register(u16),
    IntegerCompare {
        result: u16,
        lhs: u16,
        rhs: u16,
        width: WidthCode,
        predicate: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchSite {
    pub condition: Option<BranchCondition>,
    pub then_edge: BranchTarget,
    pub else_edge: Option<BranchTarget>,
}

/// One compiled function: register frame, code words and call sites.
#[derive(Clone, Debug, PartialEq)]
pub struct BcFunction {
    pub name: Name,
    pub register_count: u32,
    pub param_count: u32,
    pub returns_value: bool,
    pub code: Vec<u64>,
    pub call_sites: Vec<CallSite>,
    pub allocations: Vec<AllocShape>,
    pub branches: Vec<BranchSite>,
}

/// A module-level variable: its byte size, alignment and optional scalar
/// initializer. Allocated once at program start; `global.addr` yields its
/// address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalSpec {
    pub size: u32,
    pub align: u32,
    /// The leading bytes written over the zeroed storage; the rest stays zero.
    pub init: Vec<u8>,
}

/// How one argument or result crosses into C: an integer of a width, extended
/// by its signedness, or a float in a floating-point register.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ForeignClass {
    Signed(u16),
    Unsigned(u16),
    F32,
    F64,
}

/// The C signature of an import the program calls.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ForeignSignature {
    pub parameters: Vec<ForeignClass>,
    pub result: Option<ForeignClass>,
}

/// What the program does with an import (section 15.2, `IMPORTS`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ImportKind {
    /// A function called with this signature: a host capability of that
    /// name when the runner grants one, otherwise a native symbol.
    Call(ForeignSignature),
    /// The address of a native symbol, such as the entry C calls a
    /// bytecode function through.
    Address,
    /// A C function pointer called with this signature (section 26.4): the
    /// address is the value of the call site's `target` register, and the
    /// import names no symbol. A target that is a bytecode function's index
    /// is an ordinary indirect call instead.
    Pointer(ForeignSignature),
    /// A call of a C function with a variadic tail (section 29.5): the
    /// first `fixed` classes of the signature are its parameters and the
    /// rest the variadic arguments of this call, which the platform may
    /// pass elsewhere.
    Variadic {
        fixed: u32,
        signature: ForeignSignature,
    },
    /// A native library loaded before the program runs, its path relative
    /// to the image. Its entries number the functions they call from
    /// `first`, which is 0 for the program's own library and where a linked
    /// library's functions start.
    Library { first: u32 },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Import {
    pub symbol: String,
    pub kind: ImportKind,
}

/// A whole compiled module ready for the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub functions: Vec<BcFunction>,
    pub consts: Vec<Const>,
    pub imports: Vec<Import>,
    pub globals: Vec<GlobalSpec>,
    /// Functions run, in order, before the entry: a KIR `initializer`.
    pub initializers: Vec<usize>,
}

impl Program {
    #[must_use]
    pub fn function_index(&self, name: &str) -> Option<usize> {
        self.functions
            .iter()
            .position(|function| function.name.as_str() == name)
    }
}

/// The trace maps of a debug lowering (section 18.6): for each `HALLOCC`, by
/// function index and allocation shape, the strong references of the type
/// it allocates. They stay beside the program and out of `.klbc`, which has
/// no encoding for them.
pub type Traces = std::collections::HashMap<(usize, u16), Box<[u8]>>;
