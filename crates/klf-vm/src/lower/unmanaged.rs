//! Lowering of `select`, `fma`, the raw pointer instructions (sections 12.7
//! and 12.8) and the layout queries of section 12.16.

use super::{LowerError, Lowerer};
use crate::bytecode::{Instr, Opcode};
use klf_core::Name;
use klf_kir::{Operation, Type};

impl Lowerer<'_> {
    pub(super) fn lower_unmanaged(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::Select {
                condition,
                then_value,
                else_value,
            } => self.lower_select(results, condition, then_value, else_value),
            Operation::FloatFma {
                first,
                second,
                addend,
            } => self.lower_fma(results, first, second, addend),
            Operation::RawLoad { pointer, .. } | Operation::LoadBorrow(pointer) => {
                self.lower_load(results, pointer)
            }
            Operation::RawStore { value, pointer, .. } => self.lower_store(value, pointer),
            Operation::AddrToPointer(value) | Operation::RefUnowned(value) => {
                self.lower_copy_register(results, value)
            }
            Operation::PointerOffset { pointer, bytes } => {
                self.lower_pointer_arithmetic(results, Opcode::PtrAdd, pointer, bytes)
            }
            Operation::PointerDiff { lhs, rhs } => {
                self.lower_pointer_arithmetic(results, Opcode::PtrDiff, lhs, rhs)
            }
            Operation::TypeSize(ty) => self.lower_layout_query(results, ty, |size, _| size),
            Operation::TypeAlign(ty) => self.lower_layout_query(results, ty, |_, align| align),
            Operation::TypeStride(ty) => self.lower_layout_query(results, ty, |size, align| {
                if align == 0 {
                    size
                } else {
                    size.div_ceil(align) * align
                }
            }),
            _ => return None,
        };
        Some(result)
    }

    /// The pair of scratch registers holding `first` then `second`, which
    /// `SELECT` and `FFMA` read as `c` and `c + 1`.
    fn register_pair(&mut self, first: &Name, second: &Name) -> Result<u16, LowerError> {
        let low = self.fresh()?;
        let high = self.fresh()?;
        let first = self.register(first)?;
        let second = self.register(second)?;
        self.emit(Instr::abc(Opcode::Mov, low, first, 0));
        self.emit(Instr::abc(Opcode::Mov, high, second, 0));
        Ok(low)
    }

    fn lower_select(
        &mut self,
        results: &[Name],
        condition: &Name,
        then_value: &Name,
        else_value: &Name,
    ) -> Result<(), LowerError> {
        let pair = self.register_pair(then_value, else_value)?;
        let condition = self.register(condition)?;
        let dest = self.result(results)?;
        self.emit(Instr::abc(Opcode::Select, dest, condition, pair));
        Ok(())
    }

    fn lower_fma(
        &mut self,
        results: &[Name],
        first: &Name,
        second: &Name,
        addend: &Name,
    ) -> Result<(), LowerError> {
        let is_64 = self.float_is_64(first)?;
        let pair = self.register_pair(second, addend)?;
        let first = self.register(first)?;
        let dest = self.result(results)?;
        self.emit(Instr::abc(Opcode::FFma, dest, first, pair).with_flags(u8::from(is_64)));
        Ok(())
    }

    fn lower_copy_register(&mut self, results: &[Name], value: &Name) -> Result<(), LowerError> {
        let source = self.register(value)?;
        let dest = self.result(results)?;
        self.emit(Instr::abc(Opcode::Mov, dest, source, 0));
        Ok(())
    }

    fn lower_pointer_arithmetic(
        &mut self,
        results: &[Name],
        opcode: Opcode,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let lhs = self.register(lhs)?;
        let rhs = self.register(rhs)?;
        let dest = self.result(results)?;
        self.emit(Instr::abc(opcode, dest, lhs, rhs));
        Ok(())
    }

    fn lower_layout_query(
        &mut self,
        results: &[Name],
        ty: &Type,
        pick: fn(u64, u64) -> u64,
    ) -> Result<(), LowerError> {
        let layout = klf_layout::layout_of(ty, self.decls);
        let value = pick(layout.size, layout.align);
        self.lower_const_int(results, &Type::Integer(64), i128::from(value))
    }
}
