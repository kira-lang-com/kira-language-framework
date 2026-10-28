//! The atomics and `fence` (section 12.14). Cranelift's atomic instructions
//! are all sequentially consistent, which every weaker order the KIR asks
//! for allows.

use super::{FnLowerer, cl_type, single};
use crate::{EmitError, err};
use cranelift::codegen::ir::{AtomicRmwOp, MemFlagsData};
use cranelift::prelude::{InstBuilder, IntCC};
use klf_core::Name;
use klf_kir::{AtomicOperator, Operation};

impl FnLowerer<'_> {
    pub(super) fn atomic(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Result<(), EmitError> {
        match operation {
            Operation::AtomicLoad { ty, pointer, .. } => {
                let address = self.value(pointer)?;
                let value =
                    self.builder
                        .ins()
                        .atomic_load(cl_type(ty)?, MemFlagsData::new(), address);
                self.define(results, value)
            }
            Operation::AtomicStore { value, pointer, .. } => {
                let address = self.value(pointer)?;
                let operand = self.value(value)?;
                self.builder
                    .ins()
                    .atomic_store(MemFlagsData::new(), operand, address);
                Ok(())
            }
            Operation::AtomicRmw {
                operator,
                pointer,
                value,
                ..
            } => {
                let ty = cl_type(self.ty_of(value)?)?;
                let address = self.value(pointer)?;
                let operand = self.value(value)?;
                let previous = self.builder.ins().atomic_rmw(
                    ty,
                    MemFlagsData::new(),
                    rmw_op(*operator),
                    address,
                    operand,
                );
                let result = single(results)?.clone();
                self.values.insert(result, previous);
                Ok(())
            }
            Operation::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                ..
            } => {
                let address = self.value(pointer)?;
                let wanted = self.value(expected)?;
                let new = self.value(replacement)?;
                let previous =
                    self.builder
                        .ins()
                        .atomic_cas(MemFlagsData::new(), address, wanted, new);
                let exchanged = self.builder.ins().icmp(IntCC::Equal, previous, wanted);
                self.define_pair(results, previous, exchanged)
            }
            Operation::Fence(_) => {
                self.builder.ins().fence();
                Ok(())
            }
            other => Err(err(format!("not an atomic operation: {other:?}"))),
        }
    }
}

fn rmw_op(operator: AtomicOperator) -> AtomicRmwOp {
    match operator {
        AtomicOperator::Exchange => AtomicRmwOp::Xchg,
        AtomicOperator::Add => AtomicRmwOp::Add,
        AtomicOperator::Sub => AtomicRmwOp::Sub,
        AtomicOperator::And => AtomicRmwOp::And,
        AtomicOperator::Or => AtomicRmwOp::Or,
        AtomicOperator::Xor => AtomicRmwOp::Xor,
        AtomicOperator::Nand => AtomicRmwOp::Nand,
        AtomicOperator::Max => AtomicRmwOp::Smax,
        AtomicOperator::Min => AtomicRmwOp::Smin,
        AtomicOperator::UnsignedMax => AtomicRmwOp::Umax,
        AtomicOperator::UnsignedMin => AtomicRmwOp::Umin,
    }
}
