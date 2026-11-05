//! Printing for the section-12 instruction set.

use super::control::{write_call, write_system, write_terminator};
use super::emit::{arguments, raw_access};
use super::spelling::{
    Symbol, binary, float_binary, float_predicate_name, float_unary, load_mode, overflow_mode,
    predicate_name, store_mode,
};
use crate::{AccessEnforcement, AccessMode, Operation, Overflow, Type};
use std::fmt;

pub(crate) fn write_operation(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> fmt::Result {
    for family in [
        write_constant,
        write_arithmetic,
        write_aggregate,
        write_storage,
        write_memory,
        write_ownership,
        write_call,
        write_terminator,
        write_system,
    ] {
        if family(formatter, operation)? {
            return Ok(());
        }
    }
    Ok(())
}

fn write_constant(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::ConstInt { ty, value } => write!(formatter, "const.int {ty}, {value}"),
        Operation::ConstBool(value) => write!(formatter, "const.bool {value}"),
        Operation::ConstNull => formatter.write_str("const.null"),
        Operation::ConstFloat { ty, literal } => {
            write!(formatter, "const.float {ty}, {literal}")
        }
        Operation::ConstZero(ty) => write!(formatter, "const.zero {ty}"),
        Operation::ConstString(value) => write!(formatter, "const.string {value:?}"),
        Operation::FunctionReference(function) => {
            write!(formatter, "func.ref @{}", Symbol(function.as_str()))
        }
        Operation::ClosureCallback { user_data, ty } => write!(
            formatter,
            "closure.callback {user_data}, {}",
            Type::Function(ty.clone())
        ),
        Operation::GlobalAddress {
            global,
            thread_local,
        } => {
            let opcode = if *thread_local {
                "tls.addr"
            } else {
                "global.addr"
            };
            write!(formatter, "{opcode} @{}", Symbol(global.as_str()))
        }
        Operation::DescriptorReference(ty) => write!(formatter, "descriptor.ref {ty}"),
        Operation::WitnessReference { ty, trait_name } => {
            write!(formatter, "witness.ref {ty}, %{trait_name}")
        }
        Operation::BlockAddress(block) => write!(formatter, "block.addr {block}"),
        Operation::TypeSize(ty) => write!(formatter, "type.size {ty}"),
        Operation::TypeAlign(ty) => write!(formatter, "type.align {ty}"),
        Operation::TypeStride(ty) => write!(formatter, "type.stride {ty}"),
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn write_arithmetic(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::Binary {
            operator,
            overflow,
            signed,
            lhs,
            rhs,
        } => {
            let sign = if matches!(overflow, Overflow::Wrap) {
                ""
            } else if *signed {
                "s "
            } else {
                "u "
            };
            write!(
                formatter,
                "{} [{}] {sign}%{lhs}, %{rhs}",
                binary(*operator),
                overflow_mode(*overflow)
            )
        }
        Operation::BinaryOverflow {
            operator,
            signed,
            lhs,
            rhs,
        } => {
            let sign = if *signed { "s" } else { "u" };
            write!(formatter, "{}.ovf {sign} %{lhs}, %{rhs}", binary(*operator))
        }
        Operation::IntegerUnary { operator, value } => {
            write!(formatter, "{} %{value}", operator.spelling())
        }
        Operation::IntegerCompare {
            predicate,
            lhs,
            rhs,
        } => write!(
            formatter,
            "icmp {} %{lhs}, %{rhs}",
            predicate_name(*predicate)
        ),
        Operation::Select {
            condition,
            then_value,
            else_value,
        } => write!(
            formatter,
            "select %{condition}, %{then_value}, %{else_value}"
        ),
        Operation::FloatBinary { operator, lhs, rhs } => {
            write!(formatter, "{} %{lhs}, %{rhs}", float_binary(*operator))
        }
        Operation::FloatCompare {
            predicate,
            lhs,
            rhs,
        } => write!(
            formatter,
            "fcmp {} %{lhs}, %{rhs}",
            float_predicate_name(*predicate)
        ),
        Operation::FloatUnary { operator, value } => {
            write!(formatter, "{} %{value}", float_unary(*operator))
        }
        Operation::FloatFma {
            first,
            second,
            addend,
        } => write!(formatter, "fma %{first}, %{second}, %{addend}"),
        Operation::Convert { kind, value, ty } => {
            let saturating = if kind.is_saturating() { " [sat]" } else { "" };
            write!(
                formatter,
                "{}{saturating} %{value} to {ty}",
                kind.spelling()
            )
        }
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn write_aggregate(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::VectorSplat { ty, value } => write!(formatter, "vec.splat {ty}, %{value}"),
        Operation::VectorExtract { vector, lane } => {
            write!(formatter, "vec.extract %{vector}, {lane}")
        }
        Operation::VectorInsert {
            vector,
            lane,
            value,
        } => write!(formatter, "vec.insert %{vector}, {lane}, %{value}"),
        Operation::VectorShuffle { lhs, rhs, mask } => {
            write!(formatter, "vec.shuffle %{lhs}, %{rhs}, [")?;
            for (index, lane) in mask.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{lane}")?;
            }
            formatter.write_str("]")
        }
        Operation::Intrinsic(call) => {
            let crate::IntrinsicCall {
                name,
                arguments: values,
                result,
            } = &**call;
            write!(formatter, "intrinsic {name:?}")?;
            arguments(formatter, values)?;
            write!(formatter, " -> {result}")
        }
        Operation::StructNew { ty, fields } => {
            write!(formatter, "struct.new {ty}")?;
            arguments(formatter, fields)
        }
        Operation::StructExtract { aggregate, field } => {
            write!(formatter, "struct.extract %{aggregate}, {field}")
        }
        Operation::StructDestructure(value) => write!(formatter, "struct.destructure %{value}"),
        Operation::TupleNew(values) => {
            formatter.write_str("tuple.new")?;
            arguments(formatter, values)
        }
        Operation::TupleExtract { tuple, index } => {
            write!(formatter, "tuple.extract %{tuple}, {index}")
        }
        Operation::TupleDestructure(value) => write!(formatter, "tuple.destructure %{value}"),
        Operation::ArrayNew { element, elements } => {
            write!(formatter, "array.new {element}")?;
            arguments(formatter, elements)
        }
        Operation::EnumNew {
            ty,
            variant,
            payload,
        } => {
            write!(formatter, "enum.new {ty}, {variant}")?;
            arguments(formatter, payload)
        }
        Operation::EnumTag { value } => write!(formatter, "enum.tag %{value}"),
        Operation::EnumTake { value, variant } => {
            write!(formatter, "enum.take %{value}, {variant}")
        }
        Operation::EnumProject { value, variant } => {
            write!(formatter, "enum.project %{value}, {variant}")
        }
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn write_storage(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::StackAlloc(ty) => write!(formatter, "stack.alloc {ty}"),
        Operation::StackDealloc(value) => write!(formatter, "stack.dealloc %{value}"),
        Operation::FieldAddr { base, field } => {
            write!(formatter, "field.addr %{base}, {field}")
        }
        Operation::ElemAddr { base, index } => write!(formatter, "elem.addr %{base}, %{index}"),
        Operation::EnumPayloadAddr { base, variant } => {
            write!(formatter, "enum.payload.addr %{base}, {variant}")
        }
        Operation::EnumInject { address, variant } => {
            write!(formatter, "enum.inject %{address}, {variant}")
        }
        Operation::AddrCast { address, ty } => {
            write!(formatter, "addr.cast %{address} to {ty}")
        }
        Operation::AddrToPointer(address) => write!(formatter, "addr.toptr %{address}"),
        Operation::HeapAllocCounted(ty) => write!(formatter, "heap.alloc.counted {ty}"),
        Operation::HeapAllocUnique(ty) => write!(formatter, "heap.alloc.unique {ty}"),
        Operation::HeapAllocArray { element, count } => {
            write!(formatter, "heap.alloc.array {element}, %{count}")
        }
        Operation::HeapAllocRaw { size, align } => {
            write!(formatter, "heap.alloc.raw %{size}, %{align}")
        }
        Operation::HeapFreeRaw(pointer) => write!(formatter, "heap.free.raw %{pointer}"),
        Operation::RefAddr(value) => write!(formatter, "ref.addr %{value}"),
        Operation::UniqAddr(value) => write!(formatter, "uniq.addr %{value}"),
        Operation::WeakNew(value) => write!(formatter, "weak.new %{value}"),
        Operation::WeakLoad(value) => write!(formatter, "weak.load %{value}"),
        Operation::RefUnowned(value) => write!(formatter, "ref.unowned %{value}"),
        Operation::RefIsUnique(value) => write!(formatter, "ref.isunique %{value}"),
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn write_memory(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::Load { mode, address } => {
            write!(formatter, "load [{}] %{address}", load_mode(*mode))
        }
        Operation::LoadBorrow(address) => write!(formatter, "load.borrow %{address}"),
        Operation::Store {
            mode,
            value,
            address,
        } => write!(
            formatter,
            "store [{}] %{value} to %{address}",
            store_mode(*mode)
        ),
        Operation::CopyAddr { source, dest, mode } => write!(
            formatter,
            "copy.addr %{source} to [{}] %{dest}",
            store_mode(*mode)
        ),
        Operation::MoveAddr { source, dest, mode } => write!(
            formatter,
            "move.addr %{source} to [{}] %{dest}",
            store_mode(*mode)
        ),
        Operation::DestroyAddr(value) => write!(formatter, "destroy.addr %{value}"),
        Operation::RawLoad {
            ty,
            pointer,
            access,
        } => {
            write!(formatter, "raw.load {ty}, %{pointer}")?;
            raw_access(formatter, *access)
        }
        Operation::RawStore {
            value,
            pointer,
            access,
        } => {
            write!(formatter, "raw.store %{value}, %{pointer}")?;
            raw_access(formatter, *access)
        }
        Operation::PointerOffset { pointer, bytes } => {
            write!(formatter, "ptr.offset %{pointer}, %{bytes}")
        }
        Operation::PointerDiff { lhs, rhs } => write!(formatter, "ptr.diff %{lhs}, %{rhs}"),
        Operation::MemCopy {
            dest,
            source,
            length,
        } => write!(formatter, "mem.copy %{dest}, %{source}, %{length}"),
        Operation::MemMove {
            dest,
            source,
            length,
        } => write!(formatter, "mem.move %{dest}, %{source}, %{length}"),
        Operation::MemSet { dest, byte, length } => {
            write!(formatter, "mem.set %{dest}, %{byte}, %{length}")
        }
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn write_ownership(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::Copy(value) => write!(formatter, "copy %{value}"),
        Operation::Move(value) => write!(formatter, "move %{value}"),
        Operation::Destroy(value) => write!(formatter, "destroy %{value}"),
        Operation::BorrowBegin(value) => write!(formatter, "borrow.begin %{value}"),
        Operation::BorrowEnd(value) => write!(formatter, "borrow.end %{value}"),
        Operation::IntoRaw(value) => write!(formatter, "into.raw %{value}"),
        Operation::FromRaw { ty, pointer } => write!(formatter, "from.raw {ty}, %{pointer}"),
        Operation::AccessBegin {
            mode,
            enforcement,
            address,
        } => {
            let mode = match mode {
                AccessMode::Read => "read",
                AccessMode::Modify => "modify",
            };
            let enforcement = match enforcement {
                AccessEnforcement::Static => "static",
                AccessEnforcement::Dynamic => "dynamic",
                AccessEnforcement::Unchecked => "unchecked",
            };
            write!(formatter, "access.begin {mode} {enforcement} %{address}")
        }
        Operation::AccessEnd(token) => write!(formatter, "access.end %{token}"),
        _ => return Ok(false),
    }?;
    Ok(true)
}
