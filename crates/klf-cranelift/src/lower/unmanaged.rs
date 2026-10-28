//! `select`, `fma`, the raw pointer instructions (sections 12.7 and 12.8)
//! and the layout queries of section 12.16.

use super::{FnLowerer, cl_type};
use crate::{EmitError, err};
use cranelift::codegen::ir::MemFlagsData;
use cranelift::prelude::{InstBuilder, types};
use klf_core::Name;
use klf_kir::Operation;

impl FnLowerer<'_> {
    pub(super) fn unmanaged(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Result<(), EmitError> {
        let value = match operation {
            Operation::Select {
                condition,
                then_value,
                else_value,
            } => {
                let condition = self.value(condition)?;
                let then_value = self.value(then_value)?;
                let else_value = self.value(else_value)?;
                self.builder.ins().select(condition, then_value, else_value)
            }
            Operation::FloatFma {
                first,
                second,
                addend,
            } => {
                let first = self.value(first)?;
                let second = self.value(second)?;
                let addend = self.value(addend)?;
                self.builder.ins().fma(first, second, addend)
            }
            Operation::RawLoad { ty, pointer, .. } => {
                let address = self.value(pointer)?;
                self.builder
                    .ins()
                    .load(cl_type(ty)?, MemFlagsData::new(), address, 0)
            }
            Operation::RawStore { value, pointer, .. } => {
                let address = self.value(pointer)?;
                let operand = self.value(value)?;
                self.builder
                    .ins()
                    .store(MemFlagsData::new(), operand, address, 0);
                return Ok(());
            }
            Operation::LoadBorrow(address) => return self.load(results, address),
            Operation::AddrToPointer(value) | Operation::RefUnowned(value) => self.value(value)?,
            Operation::PointerOffset { pointer, bytes } => {
                let pointer = self.value(pointer)?;
                let bytes = self.value(bytes)?;
                self.builder.ins().iadd(pointer, bytes)
            }
            Operation::PointerDiff { lhs, rhs } => {
                let lhs = self.value(lhs)?;
                let rhs = self.value(rhs)?;
                self.builder.ins().isub(lhs, rhs)
            }
            Operation::TypeSize(ty) | Operation::TypeAlign(ty) | Operation::TypeStride(ty) => {
                let layout = self.layouts.layout_of(ty);
                let answer = match operation {
                    Operation::TypeSize(_) => layout.size,
                    Operation::TypeAlign(_) => layout.align,
                    _ => stride(layout.size, layout.align),
                };
                let answer = i64::try_from(answer).map_err(|_| err("type too large"))?;
                self.builder.ins().iconst(types::I64, answer)
            }
            other => return Err(err(format!("not an unmanaged operation: {other:?}"))),
        };
        self.define(results, value)
    }
}

/// The distance between consecutive array elements: the size rounded up to
/// the alignment.
fn stride(size: u64, align: u64) -> u64 {
    if align == 0 {
        size
    } else {
        size.div_ceil(align) * align
    }
}
