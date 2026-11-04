//! The section-12 instruction set.

use super::operators::{
    AccessEnforcement, AccessMode, AtomicOperator, BinaryOperator, ConvertKind, DebugVariable,
    FloatBinaryOperator, FloatPredicate, FloatUnaryOperator, IntegerPredicate,
    IntegerUnaryOperator, LoadMode, MemoryOrder, Overflow, StoreMode,
};
use super::types::{FunctionType, Type};
use klf_core::Name;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockTarget {
    pub block: Name,
    pub arguments: Vec<Name>,
}

/// An unmanaged raw memory access (section 12.8).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RawAccess {
    pub align: Option<u32>,
    pub volatile: bool,
}

// The operands of the operations too large to hold inline. A module holds
// tens of thousands of instructions, so every operation pays for the largest
// one held in place; these live behind a box instead.

/// `asm "template", "constraints"(args)` (section 12.14).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InlineAsm {
    pub template: String,
    pub constraints: String,
    pub arguments: Vec<Name>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntrinsicCall {
    pub name: String,
    pub arguments: Vec<Name>,
    pub result: Type,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IndirectCall {
    pub target: Name,
    pub params: Vec<Type>,
    pub result: Type,
    pub arguments: Vec<Name>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClosureCall {
    pub closure: Name,
    pub params: Vec<Type>,
    pub result: Type,
    pub arguments: Vec<Name>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WitnessCall {
    pub ty: Type,
    pub trait_name: Name,
    pub method: Name,
    pub arguments: Vec<Name>,
    pub result: Type,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VtableCall {
    pub object: Name,
    pub vtable: Name,
    pub slot: u32,
    pub arguments: Vec<Name>,
    pub result: Type,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FallibleCall {
    pub callee: Name,
    pub generic_args: Vec<Type>,
    pub arguments: Vec<Name>,
    pub normal: BlockTarget,
    pub error: BlockTarget,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnwindingCall {
    pub callee: Name,
    pub arguments: Vec<Name>,
    pub normal: BlockTarget,
    pub unwind: BlockTarget,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConditionalBranch {
    pub condition: Name,
    pub then_target: BlockTarget,
    pub else_target: BlockTarget,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IntegerSwitch {
    pub value: Name,
    pub cases: Vec<(i128, BlockTarget)>,
    pub default: BlockTarget,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnumSwitch {
    pub value: Name,
    pub cases: Vec<(u32, BlockTarget)>,
    pub default: Option<BlockTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Operation {
    // 12.1 Constants and references.
    ConstInt {
        ty: Type,
        value: i128,
    },
    ConstBool(bool),
    ConstNull,
    ConstFloat {
        ty: Type,
        literal: String,
    },
    ConstZero(Type),
    ConstString(String),
    FunctionReference(Name),
    /// `closure.callback <index>, fn(...) -> R`: a platform function whose
    /// parameter `index` is a user-data pointer to a RIDL closure record
    /// (section 21.8). Calling it calls that closure with the other
    /// arguments (section 26.4).
    ClosureCallback {
        user_data: u32,
        ty: FunctionType,
    },
    GlobalAddress {
        global: Name,
        thread_local: bool,
    },
    DescriptorReference(Type),
    WitnessReference {
        ty: Type,
        trait_name: Name,
    },
    BlockAddress(Name),

    // 12.2 Integer operations.
    Binary {
        operator: BinaryOperator,
        overflow: Overflow,
        signed: bool,
        lhs: Name,
        rhs: Name,
    },
    /// `add.ovf`, `sub.ovf`, `mul.ovf`: yields `(T, i1)`.
    BinaryOverflow {
        operator: BinaryOperator,
        signed: bool,
        lhs: Name,
        rhs: Name,
    },
    IntegerUnary {
        operator: IntegerUnaryOperator,
        value: Name,
    },
    IntegerCompare {
        predicate: IntegerPredicate,
        lhs: Name,
        rhs: Name,
    },
    Select {
        condition: Name,
        then_value: Name,
        else_value: Name,
    },

    // 12.3 Floating point.
    FloatBinary {
        operator: FloatBinaryOperator,
        lhs: Name,
        rhs: Name,
    },
    FloatCompare {
        predicate: FloatPredicate,
        lhs: Name,
        rhs: Name,
    },
    FloatUnary {
        operator: FloatUnaryOperator,
        value: Name,
    },
    /// `fma %a, %b, %c`: a fused multiply-add.
    FloatFma {
        first: Name,
        second: Name,
        addend: Name,
    },

    // 12.4 Conversions.
    Convert {
        kind: ConvertKind,
        value: Name,
        ty: Type,
    },

    // 12.5 Vectors and intrinsics.
    VectorSplat {
        ty: Type,
        value: Name,
    },
    VectorExtract {
        vector: Name,
        lane: u32,
    },
    VectorInsert {
        vector: Name,
        lane: u32,
        value: Name,
    },
    VectorShuffle {
        lhs: Name,
        rhs: Name,
        mask: Vec<u32>,
    },
    Intrinsic(Box<IntrinsicCall>),

    // 12.6 Aggregates.
    StructNew {
        ty: Type,
        fields: Vec<Name>,
    },
    StructExtract {
        aggregate: Name,
        field: u32,
    },
    /// Consumes the struct and yields every field.
    StructDestructure(Name),
    TupleNew(Vec<Name>),
    TupleExtract {
        tuple: Name,
        index: u32,
    },
    TupleDestructure(Name),
    ArrayNew {
        element: Type,
        elements: Vec<Name>,
    },
    EnumNew {
        ty: Type,
        variant: u32,
        payload: Vec<Name>,
    },
    EnumTag {
        value: Name,
    },
    /// Consumes an owned enum and yields the payload of `variant`.
    EnumTake {
        value: Name,
        variant: u32,
    },
    /// Borrowed payload of a borrowed enum.
    EnumProject {
        value: Name,
        variant: u32,
    },

    // 12.7 Storage and addresses.
    StackAlloc(Type),
    StackDealloc(Name),
    /// `field.addr %base, i`: the address of field `i` of the struct `%base`
    /// points to.
    FieldAddr {
        base: Name,
        field: u32,
    },
    /// `elem.addr %base, %index`: the address of element `%index` of the array
    /// `%base` points to.
    ElemAddr {
        base: Name,
        index: Name,
    },
    EnumPayloadAddr {
        base: Name,
        variant: u32,
    },
    EnumInject {
        address: Name,
        variant: u32,
    },
    /// `addr.cast %a to *U`, for layout-compatible types only.
    AddrCast {
        address: Name,
        ty: Type,
    },
    /// `addr.toptr %a`: the address as an unmanaged `ptr`.
    AddrToPointer(Name),

    // 12.8 Memory.
    Load {
        mode: LoadMode,
        address: Name,
    },
    /// `load.borrow %a`: borrows the stored value in place.
    LoadBorrow(Name),
    Store {
        mode: StoreMode,
        value: Name,
        address: Name,
    },
    CopyAddr {
        source: Name,
        dest: Name,
        mode: StoreMode,
    },
    MoveAddr {
        source: Name,
        dest: Name,
        mode: StoreMode,
    },
    DestroyAddr(Name),
    RawLoad {
        ty: Type,
        pointer: Name,
        access: RawAccess,
    },
    RawStore {
        value: Name,
        pointer: Name,
        access: RawAccess,
    },
    PointerOffset {
        pointer: Name,
        bytes: Name,
    },
    PointerDiff {
        lhs: Name,
        rhs: Name,
    },
    MemCopy {
        dest: Name,
        source: Name,
        length: Name,
    },
    MemMove {
        dest: Name,
        source: Name,
        length: Name,
    },
    MemSet {
        dest: Name,
        byte: Name,
        length: Name,
    },

    // 12.9 Heap and references.
    HeapAllocCounted(Type),
    HeapAllocUnique(Type),
    /// Reference-counted heap buffer of `count` elements, sized at run time.
    HeapAllocArray {
        element: Type,
        count: Name,
    },
    HeapAllocRaw {
        size: Name,
        align: Name,
    },
    HeapFreeRaw(Name),
    RefAddr(Name),
    UniqAddr(Name),
    WeakNew(Name),
    WeakLoad(Name),
    RefUnowned(Name),
    RefIsUnique(Name),

    // 12.10 Ownership.
    Copy(Name),
    Move(Name),
    Destroy(Name),
    BorrowBegin(Name),
    BorrowEnd(Name),
    IntoRaw(Name),
    FromRaw {
        ty: Type,
        pointer: Name,
    },
    AccessBegin {
        mode: AccessMode,
        enforcement: AccessEnforcement,
        address: Name,
    },
    AccessEnd(Name),

    // 12.11 Calls.
    Call {
        callee: Name,
        generic_args: Vec<Type>,
        arguments: Vec<Name>,
    },
    /// `call.indirect %fn(args) : (params) -> result`. The signature is carried
    /// explicitly so the backends can build the call without a declaration.
    CallIndirect(Box<IndirectCall>),
    /// `call.closure %c(args) : (params) -> result`; the context travels in the
    /// context register.
    CallClosure(Box<ClosureCall>),
    /// `call.witness T, Trait.method(args) -> result`.
    CallWitness(Box<WitnessCall>),
    /// `call.vtable %obj, @Vtable, slot(args) -> result`.
    CallVtable(Box<VtableCall>),
    /// `call.foreign @sym(args)`: the platform C convention. Unmanaged; the
    /// callee's signature comes from a foreign function declaration.
    CallForeign {
        symbol: Name,
        arguments: Vec<Name>,
    },
    /// `call.variadic @sym(fixed; variadic)` (section 29.5).
    CallVariadic {
        callee: Name,
        fixed: Vec<Name>,
        variadic: Vec<Name>,
    },
    /// `try.call`: a terminator that calls a fallible function and branches to
    /// `normal` with the result or `error` with the failure value.
    TryCall(Box<FallibleCall>),
    /// `invoke callee(args) to bbN unwind bbU`.
    Invoke(Box<UnwindingCall>),

    // 12.12 Unwinding.
    /// `landingpad [cleanup] [catch %typeinfo ...]`, yielding `(ptr, i32)`.
    LandingPad {
        cleanup: bool,
        catches: Vec<Name>,
    },
    Resume(Name),
    Throw(Name),

    // 12.13 Terminators.
    Branch(BlockTarget),
    ConditionalBranch(Box<ConditionalBranch>),
    SwitchInt(Box<IntegerSwitch>),
    SwitchEnum(Box<EnumSwitch>),
    /// `br.indirect %addr, [bb...]`.
    BranchIndirect {
        address: Name,
        targets: Vec<Name>,
    },
    Return(Option<Name>),
    Fail(Name),
    Trap(String),
    Unreachable,

    // 12.14 Concurrency, variadics, non-local jumps, assembly.
    AtomicLoad {
        ty: Type,
        pointer: Name,
        order: MemoryOrder,
    },
    AtomicStore {
        value: Name,
        pointer: Name,
        order: MemoryOrder,
    },
    AtomicRmw {
        operator: AtomicOperator,
        pointer: Name,
        value: Name,
        order: MemoryOrder,
    },
    /// Yields `(T, i1)`: the value read and whether the exchange succeeded.
    AtomicCompareExchange {
        weak: bool,
        pointer: Name,
        expected: Name,
        replacement: Name,
        success: MemoryOrder,
        failure: MemoryOrder,
    },
    Fence(MemoryOrder),
    VaStart(Name),
    VaArg {
        ty: Type,
        list: Name,
    },
    VaCopy {
        dest: Name,
        source: Name,
    },
    VaEnd(Name),
    /// `setjmp %buf`, in a function marked `returns_twice`.
    SetJmp(Name),
    LongJmp {
        buffer: Name,
        value: Name,
    },
    /// Inline assembly. Forces engine `native`; forbidden in `runtime`/`any`
    /// functions (verifier V9).
    Asm(Box<InlineAsm>),

    // 12.15 Debug and hints.
    DebugValue {
        value: Name,
        variable: Box<DebugVariable>,
    },
    DebugAddr {
        address: Name,
        variable: Box<DebugVariable>,
    },
    Assume(Name),
    Expect {
        value: Name,
        expected: i128,
    },
    Nop,

    // 12.16 Generic support.
    TypeSize(Type),
    TypeAlign(Type),
    TypeStride(Type),
}

#[cfg(test)]
mod tests {
    // A module holds tens of thousands of instructions, so their size is the
    // size of a module.
    #[test]
    fn instructions_stay_compact() {
        assert_eq!(std::mem::size_of::<super::Operation>(), 80);
        assert_eq!(std::mem::size_of::<crate::Instruction>(), 128);
    }
}
