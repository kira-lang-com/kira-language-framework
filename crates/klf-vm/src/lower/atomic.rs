//! Lowering of the atomics, `fence`, `setjmp` and `longjmp` (section
//! 12.14). The interpreter runs one thread, so a plain load or store is
//! already indivisible and every memory order is inert; `atomic.rmw` and
//! `atomic.cmpxchg` keep their own instructions so the read and the write
//! stay one step.

use super::{LowerError, Lowerer, err, scalar_width};
use crate::bytecode::{Instr, Opcode};
use klf_core::Name;
use klf_kir::{AtomicOperator, Operation};

/// The operator numbers `ARmw` reads from its flags, in the order
/// `numeric::atomic_apply` decodes them.
fn operator_code(operator: AtomicOperator) -> u8 {
    match operator {
        AtomicOperator::Exchange => 0,
        AtomicOperator::Add => 1,
        AtomicOperator::Sub => 2,
        AtomicOperator::And => 3,
        AtomicOperator::Or => 4,
        AtomicOperator::Xor => 5,
        AtomicOperator::Nand => 6,
        AtomicOperator::Max => 7,
        AtomicOperator::Min => 8,
        AtomicOperator::UnsignedMax => 9,
        AtomicOperator::UnsignedMin => 10,
    }
}

impl Lowerer<'_> {
    pub(super) fn lower_atomic(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::AtomicLoad { ty, pointer, .. } => scalar_width(ty).and_then(|width| {
                let dest = self.result(results)?;
                let source = self.register(pointer)?;
                self.emit(Instr::abc(Opcode::ALoad, dest, source, 0).with_flags(width.code()));
                Ok(())
            }),
            Operation::AtomicStore { value, pointer, .. } => self.atomic_store(value, pointer),
            Operation::AtomicRmw {
                operator,
                pointer,
                value,
                ..
            } => self.atomic_rmw(results, *operator, pointer, value),
            Operation::AtomicCompareExchange {
                pointer,
                expected,
                replacement,
                ..
            } => self.atomic_compare_exchange(results, pointer, expected, replacement),
            Operation::Fence(_) => {
                self.emit(Instr::abc(Opcode::Fence, 0, 0, 0));
                Ok(())
            }
            Operation::SetJmp(buffer) => self.register(buffer).and_then(|buffer| {
                let dest = self.result(results)?;
                self.emit(Instr::abc(Opcode::SetJmp, dest, buffer, 0));
                Ok(())
            }),
            Operation::LongJmp { buffer, value } => self.register(buffer).and_then(|buffer| {
                let value = self.register(value)?;
                self.emit(Instr::abc(Opcode::LongJmp, buffer, value, 0));
                Ok(())
            }),
            _ => return None,
        };
        Some(result)
    }

    fn atomic_store(&mut self, value: &Name, pointer: &Name) -> Result<(), LowerError> {
        let width = scalar_width(self.operand_type(value)?)?;
        let address = self.register(pointer)?;
        let operand = self.register(value)?;
        self.emit(Instr::abc(Opcode::AStore, address, operand, 0).with_flags(width.code()));
        Ok(())
    }

    fn atomic_rmw(
        &mut self,
        results: &[Name],
        operator: AtomicOperator,
        pointer: &Name,
        value: &Name,
    ) -> Result<(), LowerError> {
        let width = scalar_width(self.operand_type(value)?)?;
        let dest = self.result(results)?;
        let address = self.register(pointer)?;
        let operand = self.register(value)?;
        let flags = width.code() | (operator_code(operator) << 3);
        self.emit(Instr::abc(Opcode::ARmw, dest, address, operand).with_flags(flags));
        Ok(())
    }

    /// `ACas` reads the expected value and the replacement from two
    /// registers in a row and writes the value read and the flag to two
    /// more, so both pairs go through fresh registers.
    fn atomic_compare_exchange(
        &mut self,
        results: &[Name],
        pointer: &Name,
        expected: &Name,
        replacement: &Name,
    ) -> Result<(), LowerError> {
        let [read, exchanged] = results else {
            return Err(err("`atomic.cmpxchg` needs a value and a flag"));
        };
        let width = scalar_width(self.operand_type(expected)?)?;
        let address = self.register(pointer)?;
        let (wanted, new) = (self.register(expected)?, self.register(replacement)?);
        let operands = self.fresh()?;
        let second = self.fresh()?;
        self.emit(Instr::abc(Opcode::Mov, operands, wanted, 0));
        self.emit(Instr::abc(Opcode::Mov, second, new, 0));
        let answers = self.fresh()?;
        let _flag = self.fresh()?;
        self.emit(Instr::abc(Opcode::ACas, answers, address, operands).with_flags(width.code()));
        let (read, exchanged) = (self.register(read)?, self.register(exchanged)?);
        self.emit(Instr::abc(Opcode::Mov, read, answers, 0));
        self.emit(Instr::abc(Opcode::Mov, exchanged, answers + 1, 0));
        Ok(())
    }

    fn operand_type(&self, name: &Name) -> Result<&klf_kir::Type, LowerError> {
        self.types
            .get(name)
            .ok_or_else(|| err(format!("cannot type `%{name}`")))
    }
}
