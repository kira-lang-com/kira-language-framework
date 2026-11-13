//! Mutable operand and target access over [`Operation`], for passes that
//! rename values and retarget edges. It visits exactly the operands
//! `used_values` lists, in the same order.

use super::operation::{BlockTarget, Operation};
use klf_core::Name;

impl Operation {
    #[must_use]
    pub fn used_values_mut(&mut self) -> Vec<&mut Name> {
        if self.uses_single_or_none() {
            return self.single_use_mut().into_iter().collect();
        }
        self.multiple_uses_mut()
    }

    /// Visits every used value in order without allocating a temporary vector.
    #[allow(clippy::too_many_lines)]
    pub fn for_each_used_value_mut(&mut self, mut visit: impl FnMut(&mut Name)) {
        if let Some(value) = self.single_use_mut() {
            visit(value);
            return;
        }
        match self {
            Self::Binary { lhs, rhs, .. }
            | Self::BinaryOverflow { lhs, rhs, .. }
            | Self::IntegerCompare { lhs, rhs, .. }
            | Self::FloatBinary { lhs, rhs, .. }
            | Self::FloatCompare { lhs, rhs, .. }
            | Self::PointerDiff { lhs, rhs }
            | Self::VectorShuffle { lhs, rhs, .. } => {
                visit(lhs);
                visit(rhs);
            }
            Self::CopyAddr { source, dest, .. }
            | Self::MoveAddr { source, dest, .. }
            | Self::VaCopy { dest, source } => {
                visit(source);
                visit(dest);
            }
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
            } => {
                visit(value);
                visit(address);
            }
            Self::PointerOffset { pointer, bytes } => {
                visit(pointer);
                visit(bytes);
            }
            Self::HeapAllocRaw { size, align } => {
                visit(size);
                visit(align);
            }
            Self::ElemAddr { base, index } => {
                visit(base);
                visit(index);
            }
            Self::AtomicLoad { pointer, .. } | Self::FieldAddr { base: pointer, .. } => {
                visit(pointer);
            }
            Self::AtomicRmw { pointer, value, .. } => {
                visit(pointer);
                visit(value);
            }
            Self::VectorInsert { vector, value, .. } => {
                visit(vector);
                visit(value);
            }
            Self::Select {
                condition,
                then_value,
                else_value,
            } => {
                visit(condition);
                visit(then_value);
                visit(else_value);
            }
            Self::FloatFma {
                first,
                second,
                addend,
            } => {
                visit(first);
                visit(second);
                visit(addend);
            }
            Self::MemCopy {
                dest,
                source,
                length,
            }
            | Self::MemMove {
                dest,
                source,
                length,
            } => {
                visit(dest);
                visit(source);
                visit(length);
            }
            Self::MemSet { dest, byte, length } => {
                visit(dest);
                visit(byte);
                visit(length);
            }
            Self::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                ..
            } => {
                visit(pointer);
                visit(expected);
                visit(replacement);
            }
            Self::Call { arguments, .. } | Self::CallForeign { arguments, .. } => {
                for value in arguments {
                    visit(value);
                }
            }
            Self::CallWitness(call) => {
                for value in &mut call.arguments {
                    visit(value);
                }
            }
            Self::Intrinsic(call) => {
                for value in &mut call.arguments {
                    visit(value);
                }
            }
            Self::StructNew { fields, .. }
            | Self::TupleNew(fields)
            | Self::ArrayNew {
                elements: fields, ..
            }
            | Self::EnumNew {
                payload: fields, ..
            }
            | Self::LandingPad {
                catches: fields, ..
            } => {
                for value in fields {
                    visit(value);
                }
            }
            Self::CallIndirect(call) => {
                visit(&mut call.target);
                for value in &mut call.arguments {
                    visit(value);
                }
            }
            Self::CallClosure(call) => {
                visit(&mut call.closure);
                for value in &mut call.arguments {
                    visit(value);
                }
            }
            Self::CallVtable(call) => {
                visit(&mut call.object);
                for value in &mut call.arguments {
                    visit(value);
                }
            }
            Self::CallVariadic {
                fixed, variadic, ..
            } => {
                for value in fixed.iter_mut().chain(variadic) {
                    visit(value);
                }
            }
            Self::Asm(asm) => {
                for value in &mut asm.arguments {
                    visit(value);
                }
            }
            Self::TryCall(call) => {
                let call = &mut **call;
                for value in call
                    .arguments
                    .iter_mut()
                    .chain(&mut call.normal.arguments)
                    .chain(&mut call.error.arguments)
                {
                    visit(value);
                }
            }
            Self::Invoke(call) => {
                let call = &mut **call;
                for value in call
                    .arguments
                    .iter_mut()
                    .chain(&mut call.normal.arguments)
                    .chain(&mut call.unwind.arguments)
                {
                    visit(value);
                }
            }
            Self::Branch(target) => {
                for value in &mut target.arguments {
                    visit(value);
                }
            }
            Self::BranchIndirect { address, .. } => visit(address),
            Self::ConditionalBranch(branch) => {
                let branch = &mut **branch;
                visit(&mut branch.condition);
                for value in branch
                    .then_target
                    .arguments
                    .iter_mut()
                    .chain(&mut branch.else_target.arguments)
                {
                    visit(value);
                }
            }
            Self::SwitchInt(switch) => {
                let switch = &mut **switch;
                visit(&mut switch.value);
                for (_, target) in &mut switch.cases {
                    for value in &mut target.arguments {
                        visit(value);
                    }
                }
                for value in &mut switch.default.arguments {
                    visit(value);
                }
            }
            Self::SwitchEnum(switch) => {
                let switch = &mut **switch;
                visit(&mut switch.value);
                for (_, target) in &mut switch.cases {
                    for value in &mut target.arguments {
                        visit(value);
                    }
                }
                if let Some(target) = &mut switch.default {
                    for value in &mut target.arguments {
                        visit(value);
                    }
                }
            }
            Self::Return(value) => {
                if let Some(value) = value {
                    visit(value);
                }
            }
            _ => {}
        }
    }

    fn uses_single_or_none(&self) -> bool {
        self.nullary_or_unary_uses().is_some()
    }

    fn single_use_mut(&mut self) -> Option<&mut Name> {
        match self {
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
            | Self::Fail(value) => Some(value),
            _ => None,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn multiple_uses_mut(&mut self) -> Vec<&mut Name> {
        match self {
            Self::Binary { lhs, rhs, .. }
            | Self::BinaryOverflow { lhs, rhs, .. }
            | Self::IntegerCompare { lhs, rhs, .. }
            | Self::FloatBinary { lhs, rhs, .. }
            | Self::FloatCompare { lhs, rhs, .. }
            | Self::PointerDiff { lhs, rhs }
            | Self::VectorShuffle { lhs, rhs, .. } => vec![lhs, rhs],
            Self::CopyAddr { source, dest, .. }
            | Self::MoveAddr { source, dest, .. }
            | Self::VaCopy { dest, source } => vec![source, dest],
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
            } => vec![value, address],
            Self::PointerOffset { pointer, bytes } => vec![pointer, bytes],
            Self::HeapAllocRaw { size, align } => vec![size, align],
            Self::ElemAddr { base, index } => vec![base, index],
            Self::AtomicLoad { pointer, .. } => vec![pointer],
            Self::AtomicRmw { pointer, value, .. } => vec![pointer, value],
            Self::FieldAddr { base, .. } => vec![base],
            Self::VectorInsert { vector, value, .. } => vec![vector, value],
            Self::Select {
                condition,
                then_value,
                else_value,
            } => vec![condition, then_value, else_value],
            Self::FloatFma {
                first,
                second,
                addend,
            } => vec![first, second, addend],
            Self::MemCopy {
                dest,
                source,
                length,
            }
            | Self::MemMove {
                dest,
                source,
                length,
            } => vec![dest, source, length],
            Self::MemSet { dest, byte, length } => vec![dest, byte, length],
            Self::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                ..
            } => vec![pointer, expected, replacement],
            Self::Call { arguments, .. } | Self::CallForeign { arguments, .. } => {
                arguments.iter_mut().collect()
            }
            Self::CallWitness(call) => call.arguments.iter_mut().collect(),
            Self::Intrinsic(call) => call.arguments.iter_mut().collect(),
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
            } => values.iter_mut().collect(),
            Self::CallIndirect(call) => std::iter::once(&mut call.target)
                .chain(&mut call.arguments)
                .collect(),
            Self::CallClosure(call) => std::iter::once(&mut call.closure)
                .chain(&mut call.arguments)
                .collect(),
            Self::CallVtable(call) => std::iter::once(&mut call.object)
                .chain(&mut call.arguments)
                .collect(),
            Self::CallVariadic {
                fixed, variadic, ..
            } => fixed.iter_mut().chain(variadic).collect(),
            Self::Asm(asm) => asm.arguments.iter_mut().collect(),
            Self::TryCall(call) => {
                let call = &mut **call;
                call.arguments
                    .iter_mut()
                    .chain(&mut call.normal.arguments)
                    .chain(&mut call.error.arguments)
                    .collect()
            }
            Self::Invoke(call) => {
                let call = &mut **call;
                call.arguments
                    .iter_mut()
                    .chain(&mut call.normal.arguments)
                    .chain(&mut call.unwind.arguments)
                    .collect()
            }
            Self::Branch(target) => target.arguments.iter_mut().collect(),
            Self::BranchIndirect { address, .. } => vec![address],
            Self::ConditionalBranch(branch) => {
                let branch = &mut **branch;
                std::iter::once(&mut branch.condition)
                    .chain(&mut branch.then_target.arguments)
                    .chain(&mut branch.else_target.arguments)
                    .collect()
            }
            Self::SwitchInt(switch) => {
                let switch = &mut **switch;
                std::iter::once(&mut switch.value)
                    .chain(
                        switch
                            .cases
                            .iter_mut()
                            .flat_map(|(_, target)| target.arguments.iter_mut()),
                    )
                    .chain(&mut switch.default.arguments)
                    .collect()
            }
            Self::SwitchEnum(switch) => {
                let switch = &mut **switch;
                std::iter::once(&mut switch.value)
                    .chain(
                        switch
                            .cases
                            .iter_mut()
                            .flat_map(|(_, target)| target.arguments.iter_mut()),
                    )
                    .chain(
                        switch
                            .default
                            .iter_mut()
                            .flat_map(|target| target.arguments.iter_mut()),
                    )
                    .collect()
            }
            Self::Return(value) => value.iter_mut().collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn targets_mut(&mut self) -> Vec<&mut BlockTarget> {
        match self {
            Self::TryCall(call) => {
                let call = &mut **call;
                vec![&mut call.normal, &mut call.error]
            }
            Self::Invoke(call) => {
                let call = &mut **call;
                vec![&mut call.normal, &mut call.unwind]
            }
            Self::Branch(target) => vec![target],
            Self::ConditionalBranch(branch) => {
                let branch = &mut **branch;
                vec![&mut branch.then_target, &mut branch.else_target]
            }
            Self::SwitchInt(switch) => {
                let switch = &mut **switch;
                switch
                    .cases
                    .iter_mut()
                    .map(|(_, target)| target)
                    .chain(std::iter::once(&mut switch.default))
                    .collect()
            }
            Self::SwitchEnum(switch) => {
                let switch = &mut **switch;
                switch
                    .cases
                    .iter_mut()
                    .map(|(_, target)| target)
                    .chain(switch.default.iter_mut())
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}
