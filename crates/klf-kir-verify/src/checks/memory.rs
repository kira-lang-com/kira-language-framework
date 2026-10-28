//! V2 rules for storage, memory, heap and ownership: sections 12.7 through
//! 12.10.

use super::{Context, require};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Instruction, Operation, Type};

pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_places(context, instruction, diagnostics)
        || verify_ownership(context, instruction, diagnostics)
}

/// Addressing, loads and stores: the operations that reach through an address.
fn verify_places(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::StackDealloc(address) => {
            require(
                matches!(context.ty(address), Some(Type::Address(_))),
                "`stack.dealloc` requires a stack address",
                instruction,
                diagnostics,
            );
        }
        Operation::FieldAddr { base, .. } => {
            require(
                context.pointee(base).is_some(),
                "`field.addr` requires an address base",
                instruction,
                diagnostics,
            );
        }
        Operation::ElemAddr { base, index } => {
            require(
                context.pointee(base).is_some(),
                "`elem.addr` requires an address base",
                instruction,
                diagnostics,
            );
            require(
                context.is_integer(index),
                "`elem.addr` index must be an integer",
                instruction,
                diagnostics,
            );
        }
        Operation::AddrCast { address, .. } | Operation::AddrToPointer(address) => {
            require(
                context.pointee(address).is_some(),
                "address conversion requires an address operand",
                instruction,
                diagnostics,
            );
        }
        Operation::Load { address, .. } | Operation::LoadBorrow(address) => {
            require(
                context.pointee(address).is_some(),
                "`load` requires an address, reference or unique operand",
                instruction,
                diagnostics,
            );
        }
        Operation::Store { value, address, .. } => {
            let target = context.pointee(address);
            require(
                target.is_some() && target == context.ty(value),
                "`store` value type must match the address pointee",
                instruction,
                diagnostics,
            );
        }
        Operation::CopyAddr { source, dest, .. } | Operation::MoveAddr { source, dest, .. } => {
            let from = context.addressed(source);
            require(
                from.is_some() && from == context.addressed(dest),
                "address-to-address copy requires two addresses of one type",
                instruction,
                diagnostics,
            );
        }
        Operation::DestroyAddr(address) => {
            require(
                context.addressed(address).is_some(),
                "`destroy.addr` requires an address operand",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

/// Raw pointers, allocation and the reference-counting operations.
fn verify_ownership(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_heap(context, instruction, diagnostics)
        || verify_references(context, instruction, diagnostics)
}

/// Heap allocation, raw pointers and the bulk memory operations.
fn verify_heap(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::RawLoad { pointer, .. } => {
            raw_pointer(context, pointer, "`raw.load`", instruction, diagnostics);
        }
        Operation::RawStore { value, pointer, .. } => {
            raw_pointer(context, pointer, "`raw.store`", instruction, diagnostics);
            require(
                context.ty(value).is_some(),
                "`raw.store` value has no known type",
                instruction,
                diagnostics,
            );
        }
        Operation::PointerOffset { pointer, bytes } => {
            raw_pointer(context, pointer, "`ptr.offset`", instruction, diagnostics);
            require(
                context.is_integer(bytes),
                "`ptr.offset` displacement must be an integer",
                instruction,
                diagnostics,
            );
        }
        Operation::PointerDiff { lhs, rhs } => {
            raw_pointer(context, lhs, "`ptr.diff`", instruction, diagnostics);
            raw_pointer(context, rhs, "`ptr.diff`", instruction, diagnostics);
        }
        Operation::MemCopy {
            dest,
            source,
            length,
        }
        | Operation::MemMove {
            dest,
            source,
            length,
        } => {
            raw_pointer(context, dest, "bulk memory", instruction, diagnostics);
            raw_pointer(context, source, "bulk memory", instruction, diagnostics);
            require(
                context.is_integer(length),
                "bulk memory length must be an integer",
                instruction,
                diagnostics,
            );
        }
        Operation::MemSet { dest, byte, length } => {
            raw_pointer(context, dest, "`mem.set`", instruction, diagnostics);
            require(
                context.is_integer(byte) && context.is_integer(length),
                "`mem.set` fill byte and length must be integers",
                instruction,
                diagnostics,
            );
        }
        Operation::HeapAllocArray { count, .. } => {
            require(
                context.is_integer(count),
                "`heap.alloc.array` count must be an integer",
                instruction,
                diagnostics,
            );
        }
        Operation::HeapAllocRaw { size, align } => {
            require(
                context.is_integer(size) && context.is_integer(align),
                "`heap.alloc.raw` size and alignment must be integers",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

/// The reference, unique and weak operations of section 12.10.
fn verify_references(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::HeapFreeRaw(pointer) | Operation::FromRaw { pointer, .. } => {
            raw_pointer(
                context,
                pointer,
                "raw heap access",
                instruction,
                diagnostics,
            );
        }
        Operation::RefAddr(reference) => {
            require(
                matches!(context.ty(reference), Some(Type::Reference(_))),
                "`ref.addr` requires a `ref T`",
                instruction,
                diagnostics,
            );
        }
        Operation::UniqAddr(unique) => {
            require(
                matches!(context.ty(unique), Some(Type::Unique(_))),
                "`uniq.addr` requires a `uniq T`",
                instruction,
                diagnostics,
            );
        }
        Operation::WeakNew(reference) | Operation::RefUnowned(reference) => {
            require(
                matches!(context.ty(reference), Some(Type::Reference(_))),
                "weak and unowned references are formed from a `ref T`",
                instruction,
                diagnostics,
            );
        }
        Operation::WeakLoad(weak) => {
            require(
                matches!(context.ty(weak), Some(Type::Weak(_))),
                "`weak.load` requires a `weak T`",
                instruction,
                diagnostics,
            );
        }
        Operation::RefIsUnique(reference) => {
            require(
                matches!(
                    context.ty(reference),
                    Some(Type::Reference(_) | Type::Unique(_))
                ),
                "`ref.isunique` requires a `ref T` or `uniq T`",
                instruction,
                diagnostics,
            );
        }
        Operation::IntoRaw(value) => {
            require(
                context.pointee(value).is_some(),
                "`into.raw` requires an owning reference or address",
                instruction,
                diagnostics,
            );
        }
        Operation::StackAlloc(_)
        | Operation::HeapAllocCounted(_)
        | Operation::HeapAllocUnique(_)
        | Operation::Copy(_)
        | Operation::Move(_)
        | Operation::Destroy(_)
        | Operation::BorrowBegin(_)
        | Operation::BorrowEnd(_)
        | Operation::AccessBegin { .. }
        | Operation::AccessEnd(_) => {}
        _ => return false,
    }
    true
}

fn raw_pointer(
    context: &Context<'_>,
    pointer: &Name,
    what: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    require(
        context.is_pointer(pointer),
        format!("{what} requires an unmanaged `ptr` operand"),
        instruction,
        diagnostics,
    );
}
