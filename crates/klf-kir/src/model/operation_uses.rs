//! Operand, successor and result-shape queries over [`Operation`].

use super::operation::{BlockTarget, Operation};
use klf_core::Name;

/// How many SSA results an instruction defines (verifier rule V2).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResultArity {
    /// A count fixed by the opcode alone.
    Fixed(usize),
    /// A count the opcode alone does not fix: it follows a callee's signature
    /// or a type declaration, and the verifier checks it there.
    Declared,
}

impl Operation {
    #[must_use]
    pub fn is_terminator(&self) -> bool {
        matches!(
            self,
            Self::TryCall { .. }
                | Self::Invoke { .. }
                | Self::Branch(_)
                | Self::ConditionalBranch { .. }
                | Self::SwitchInt { .. }
                | Self::SwitchEnum { .. }
                | Self::BranchIndirect { .. }
                | Self::Return(_)
                | Self::Fail(_)
                | Self::Trap(_)
                | Self::Resume(_)
                | Self::Throw(_)
                | Self::Unreachable
        )
    }

    /// Whether this is an unmanaged instruction (section 11.5): the verifier
    /// stops tracking ownership through it and a function containing one is
    /// recorded in the module's unmanaged manifest.
    #[must_use]
    pub fn is_unmanaged(&self) -> bool {
        matches!(
            self,
            Self::Asm(_)
                | Self::CallForeign { .. }
                | Self::CallVariadic { .. }
                | Self::IntoRaw(_)
                | Self::FromRaw { .. }
                | Self::RawLoad { .. }
                | Self::RawStore { .. }
                | Self::PointerOffset { .. }
                | Self::PointerDiff { .. }
                | Self::HeapAllocRaw { .. }
                | Self::HeapFreeRaw(_)
                | Self::AddrToPointer(_)
        )
    }

    #[must_use]
    pub fn result_arity(&self) -> ResultArity {
        match self {
            Self::ConstString(_)
            | Self::BinaryOverflow { .. }
            | Self::AtomicCompareExchange { .. }
            | Self::LandingPad { .. } => ResultArity::Fixed(2),
            Self::Call { .. }
            | Self::CallIndirect { .. }
            | Self::CallClosure { .. }
            | Self::CallWitness { .. }
            | Self::CallVtable { .. }
            | Self::CallForeign { .. }
            | Self::CallVariadic { .. }
            | Self::StructDestructure(_)
            | Self::TupleDestructure(_)
            | Self::EnumTake { .. }
            | Self::EnumProject { .. }
            | Self::Intrinsic { .. } => ResultArity::Declared,
            operation if operation.produces_value() => ResultArity::Fixed(1),
            _ => ResultArity::Fixed(0),
        }
    }

    /// Whether the opcode defines exactly one SSA result.
    #[must_use]
    fn produces_value(&self) -> bool {
        matches!(
            self,
            Self::ConstInt { .. }
                | Self::ConstBool(_)
                | Self::ConstNull
                | Self::ConstFloat { .. }
                | Self::ConstZero(_)
                | Self::FunctionReference(_)
                | Self::ClosureCallback { .. }
                | Self::GlobalAddress { .. }
                | Self::DescriptorReference(_)
                | Self::WitnessReference { .. }
                | Self::BlockAddress(_)
                | Self::Binary { .. }
                | Self::IntegerUnary { .. }
                | Self::IntegerCompare { .. }
                | Self::Select { .. }
                | Self::FloatBinary { .. }
                | Self::FloatCompare { .. }
                | Self::FloatUnary { .. }
                | Self::FloatFma { .. }
                | Self::Convert { .. }
                | Self::VectorSplat { .. }
                | Self::VectorExtract { .. }
                | Self::VectorInsert { .. }
                | Self::VectorShuffle { .. }
                | Self::StructNew { .. }
                | Self::StructExtract { .. }
                | Self::TupleNew(_)
                | Self::TupleExtract { .. }
                | Self::ArrayNew { .. }
                | Self::EnumNew { .. }
                | Self::EnumTag { .. }
                | Self::StackAlloc(_)
                | Self::FieldAddr { .. }
                | Self::ElemAddr { .. }
                | Self::EnumPayloadAddr { .. }
                | Self::AddrCast { .. }
                | Self::AddrToPointer(_)
                | Self::Load { .. }
                | Self::LoadBorrow(_)
                | Self::RawLoad { .. }
                | Self::PointerOffset { .. }
                | Self::PointerDiff { .. }
                | Self::HeapAllocCounted(_)
                | Self::HeapAllocUnique(_)
                | Self::HeapAllocArray { .. }
                | Self::HeapAllocRaw { .. }
                | Self::RefAddr(_)
                | Self::UniqAddr(_)
                | Self::WeakNew(_)
                | Self::WeakLoad(_)
                | Self::RefUnowned(_)
                | Self::RefIsUnique(_)
                | Self::Copy(_)
                | Self::Move(_)
                | Self::BorrowBegin(_)
                | Self::IntoRaw(_)
                | Self::FromRaw { .. }
                | Self::AccessBegin { .. }
                | Self::AtomicLoad { .. }
                | Self::AtomicRmw { .. }
                | Self::VaArg { .. }
                | Self::SetJmp(_)
                | Self::Expect { .. }
                | Self::TypeSize(_)
                | Self::TypeAlign(_)
                | Self::TypeStride(_)
        )
    }

    #[must_use]
    pub fn used_values(&self) -> Vec<&Name> {
        if let Some(values) = self.nullary_or_unary_uses() {
            return values;
        }
        if let Some(values) = self.binary_uses() {
            return values;
        }
        self.variadic_uses()
    }

    /// Operands of the instructions that take none or exactly one value.
    pub(super) fn nullary_or_unary_uses(&self) -> Option<Vec<&Name>> {
        let single = match self {
            Self::ConstInt { .. }
            | Self::ConstBool(_)
            | Self::ConstNull
            | Self::ConstFloat { .. }
            | Self::ConstZero(_)
            | Self::ConstString(_)
            | Self::FunctionReference(_)
            | Self::ClosureCallback { .. }
            | Self::GlobalAddress { .. }
            | Self::DescriptorReference(_)
            | Self::WitnessReference { .. }
            | Self::BlockAddress(_)
            | Self::StackAlloc(_)
            | Self::HeapAllocCounted(_)
            | Self::HeapAllocUnique(_)
            | Self::Trap(_)
            | Self::Unreachable
            | Self::Fence(_)
            | Self::Nop
            | Self::TypeSize(_)
            | Self::TypeAlign(_)
            | Self::TypeStride(_) => return Some(Vec::new()),
            Self::HeapAllocArray { count: value, .. }
            | Self::IntegerUnary { value, .. }
            | Self::FloatUnary { value, .. }
            | Self::StructExtract {
                aggregate: value, ..
            }
            | Self::StructDestructure(value)
            | Self::TupleExtract { tuple: value, .. }
            | Self::TupleDestructure(value)
            | Self::EnumTag { value }
            | Self::EnumTake { value, .. }
            | Self::EnumProject { value, .. }
            | Self::EnumPayloadAddr { base: value, .. }
            | Self::EnumInject { address: value, .. }
            | Self::VectorExtract { vector: value, .. }
            | Self::VectorSplat { value, .. }
            | Self::AddrCast { address: value, .. }
            | Self::AddrToPointer(value)
            | Self::Convert { value, .. }
            | Self::StackDealloc(value)
            | Self::Load { address: value, .. }
            | Self::LoadBorrow(value)
            | Self::RawLoad { pointer: value, .. }
            | Self::HeapFreeRaw(value)
            | Self::RefAddr(value)
            | Self::UniqAddr(value)
            | Self::WeakNew(value)
            | Self::WeakLoad(value)
            | Self::RefUnowned(value)
            | Self::RefIsUnique(value)
            | Self::Copy(value)
            | Self::Move(value)
            | Self::Destroy(value)
            | Self::BorrowBegin(value)
            | Self::BorrowEnd(value)
            | Self::IntoRaw(value)
            | Self::FromRaw { pointer: value, .. }
            | Self::AccessBegin { address: value, .. }
            | Self::AccessEnd(value)
            | Self::DestroyAddr(value)
            | Self::VaStart(value)
            | Self::VaArg { list: value, .. }
            | Self::VaEnd(value)
            | Self::SetJmp(value)
            | Self::DebugValue { value, .. }
            | Self::DebugAddr { address: value, .. }
            | Self::Assume(value)
            | Self::Expect { value, .. }
            | Self::Resume(value)
            | Self::Throw(value)
            | Self::Fail(value) => value,
            _ => return None,
        };
        Some(vec![single])
    }

    /// Operands of the instructions that take exactly two or three values.
    fn binary_uses(&self) -> Option<Vec<&Name>> {
        let pair = match self {
            Self::Binary { lhs, rhs, .. }
            | Self::BinaryOverflow { lhs, rhs, .. }
            | Self::IntegerCompare { lhs, rhs, .. }
            | Self::FloatBinary { lhs, rhs, .. }
            | Self::FloatCompare { lhs, rhs, .. }
            | Self::PointerDiff { lhs, rhs }
            | Self::VectorShuffle { lhs, rhs, .. } => (lhs, rhs),
            Self::CopyAddr { source, dest, .. }
            | Self::MoveAddr { source, dest, .. }
            | Self::VaCopy { dest, source } => (source, dest),
            Self::Store { value, address, .. }
            | Self::RawStore {
                value,
                pointer: address,
                ..
            }
            | Self::AtomicStore {
                value,
                pointer: address,
                ..
            }
            | Self::LongJmp {
                value,
                buffer: address,
            } => (value, address),
            Self::PointerOffset { pointer, bytes } => (pointer, bytes),
            Self::HeapAllocRaw { size, align } => (size, align),
            Self::ElemAddr { base, index } => (base, index),
            Self::AtomicLoad { pointer, .. } => return Some(vec![pointer]),
            Self::AtomicRmw { pointer, value, .. } => (pointer, value),
            Self::FieldAddr { base, .. } => return Some(vec![base]),
            Self::VectorInsert { vector, value, .. } => (vector, value),
            Self::Select {
                condition,
                then_value,
                else_value,
            } => return Some(vec![condition, then_value, else_value]),
            Self::FloatFma {
                first,
                second,
                addend,
            } => return Some(vec![first, second, addend]),
            Self::MemCopy {
                dest,
                source,
                length,
            }
            | Self::MemMove {
                dest,
                source,
                length,
            } => return Some(vec![dest, source, length]),
            Self::MemSet { dest, byte, length } => return Some(vec![dest, byte, length]),
            Self::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                ..
            } => return Some(vec![pointer, expected, replacement]),
            _ => return None,
        };
        Some(vec![pair.0, pair.1])
    }

    /// Operands of the instructions whose operand count varies.
    fn variadic_uses(&self) -> Vec<&Name> {
        match self {
            Self::Call { arguments, .. } | Self::CallForeign { arguments, .. } => {
                arguments.iter().collect()
            }
            Self::CallWitness(call) => call.arguments.iter().collect(),
            Self::Intrinsic(call) => call.arguments.iter().collect(),
            Self::StructNew { fields: values, .. }
            | Self::TupleNew(values)
            | Self::ArrayNew {
                elements: values, ..
            }
            | Self::EnumNew {
                payload: values, ..
            }
            | Self::LandingPad {
                catches: values, ..
            } => values.iter().collect(),
            Self::CallIndirect(call) => std::iter::once(&call.target)
                .chain(&call.arguments)
                .collect(),
            Self::CallClosure(call) => std::iter::once(&call.closure)
                .chain(&call.arguments)
                .collect(),
            Self::CallVtable(call) => std::iter::once(&call.object)
                .chain(&call.arguments)
                .collect(),
            Self::CallVariadic {
                fixed, variadic, ..
            } => fixed.iter().chain(variadic).collect(),
            Self::Asm(asm) => asm.arguments.iter().collect(),
            Self::TryCall(call) => call
                .arguments
                .iter()
                .chain(&call.normal.arguments)
                .chain(&call.error.arguments)
                .collect(),
            Self::Invoke(call) => call
                .arguments
                .iter()
                .chain(&call.normal.arguments)
                .chain(&call.unwind.arguments)
                .collect(),
            Self::Branch(target) => target.arguments.iter().collect(),
            Self::BranchIndirect { address, .. } => vec![address],
            Self::ConditionalBranch(branch) => std::iter::once(&branch.condition)
                .chain(&branch.then_target.arguments)
                .chain(&branch.else_target.arguments)
                .collect(),
            Self::SwitchInt(switch) => std::iter::once(&switch.value)
                .chain(
                    switch
                        .cases
                        .iter()
                        .flat_map(|(_, target)| target.arguments.iter()),
                )
                .chain(&switch.default.arguments)
                .collect(),
            Self::SwitchEnum(switch) => std::iter::once(&switch.value)
                .chain(
                    switch
                        .cases
                        .iter()
                        .flat_map(|(_, target)| target.arguments.iter()),
                )
                .chain(
                    switch
                        .default
                        .iter()
                        .flat_map(|target| target.arguments.iter()),
                )
                .collect(),
            Self::Return(value) => value.iter().collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn targets(&self) -> Vec<&BlockTarget> {
        match self {
            Self::TryCall(call) => vec![&call.normal, &call.error],
            Self::Invoke(call) => vec![&call.normal, &call.unwind],
            Self::Branch(target) => vec![target],
            Self::ConditionalBranch(branch) => vec![&branch.then_target, &branch.else_target],
            Self::SwitchInt(switch) => switch
                .cases
                .iter()
                .map(|(_, target)| target)
                .chain(std::iter::once(&switch.default))
                .collect(),
            Self::SwitchEnum(switch) => switch
                .cases
                .iter()
                .map(|(_, target)| target)
                .chain(switch.default.iter())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Successor block names, including `br.indirect`, whose targets carry no
    /// arguments and so are not [`BlockTarget`]s.
    #[must_use]
    pub fn successors(&self) -> Vec<&Name> {
        match self {
            Self::BranchIndirect { targets, .. } => targets.iter().collect(),
            other => other.targets().iter().map(|target| &target.block).collect(),
        }
    }
}
