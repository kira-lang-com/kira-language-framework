//! The atomics, `fence`, `setjmp`, `longjmp` and `asm` (section 12.14).
//! Atomics are LLVM's own atomic instructions on naturally aligned memory.
//! An order LLVM does not allow where it stands becomes the nearest
//! stronger one. `setjmp` and `longjmp` are the C library's, the first
//! called as returning twice so LLVM keeps every value the second return
//! needs.

use super::FnEmitter;
use crate::EmitError;
use crate::lltype::{llvm_type, scalar_bits};
use klf_core::Name;
use klf_kir::{AtomicOperator, MemoryOrder, Operation, Type};
use std::fmt::Write as _;

/// LLVM's name for `order`.
fn spelling(order: MemoryOrder) -> &'static str {
    match order {
        MemoryOrder::Relaxed => "monotonic",
        MemoryOrder::Acquire => "acquire",
        MemoryOrder::Release => "release",
        MemoryOrder::AcquireRelease => "acq_rel",
        MemoryOrder::SequentiallyConsistent => "seq_cst",
    }
}

/// A load neither releases nor, alone, acquires and releases.
fn load_order(order: MemoryOrder) -> &'static str {
    match order {
        MemoryOrder::Release | MemoryOrder::AcquireRelease => "seq_cst",
        other => spelling(other),
    }
}

/// A store does not acquire.
fn store_order(order: MemoryOrder) -> &'static str {
    match order {
        MemoryOrder::Acquire | MemoryOrder::AcquireRelease => "seq_cst",
        other => spelling(other),
    }
}

/// What a compare-exchange that fails orders: it only reads.
fn failure_order(order: MemoryOrder) -> &'static str {
    match order {
        MemoryOrder::Release => "monotonic",
        MemoryOrder::AcquireRelease => "acquire",
        other => spelling(other),
    }
}

fn rmw_spelling(operator: AtomicOperator) -> &'static str {
    match operator {
        AtomicOperator::Exchange => "xchg",
        AtomicOperator::Add => "add",
        AtomicOperator::Sub => "sub",
        AtomicOperator::And => "and",
        AtomicOperator::Or => "or",
        AtomicOperator::Xor => "xor",
        AtomicOperator::Nand => "nand",
        AtomicOperator::Max => "max",
        AtomicOperator::Min => "min",
        AtomicOperator::UnsignedMax => "umax",
        AtomicOperator::UnsignedMin => "umin",
    }
}

/// The LLVM type of `ty` and its natural alignment in bytes.
fn typed(ty: &Type) -> Result<(String, u32), EmitError> {
    Ok((llvm_type(ty)?, scalar_bits(ty)?.div_ceil(8).max(1)))
}

impl FnEmitter<'_, '_> {
    pub(super) fn atomic(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), EmitError>> {
        Some(match operation {
            Operation::AtomicLoad { ty, pointer, order } => {
                self.atomic_load(results, ty, pointer, *order)
            }
            Operation::AtomicStore {
                value,
                pointer,
                order,
            } => self.atomic_store(value, pointer, *order),
            Operation::AtomicRmw {
                operator,
                pointer,
                value,
                order,
            } => self.atomic_rmw(results, *operator, pointer, value, *order),
            Operation::AtomicCompareExchange {
                weak,
                pointer,
                expected,
                replacement,
                success,
                failure,
            } => self.compare_exchange(
                results,
                *weak,
                pointer,
                expected,
                replacement,
                (*success, *failure),
            ),
            Operation::Fence(MemoryOrder::Relaxed) => Ok(()),
            Operation::Fence(order) => {
                self.line(&format!("  fence {}", spelling(*order)));
                Ok(())
            }
            Operation::Asm(asm) => self.inline_asm(results, asm),
            Operation::SetJmp(buffer) => self.set_jump(results, buffer),
            Operation::LongJmp { buffer, value } => self.long_jump(buffer, value),
            _ => return None,
        })
    }

    fn atomic_load(
        &mut self,
        results: &[Name],
        ty: &Type,
        pointer: &Name,
        order: MemoryOrder,
    ) -> Result<(), EmitError> {
        let (llvm, align) = typed(ty)?;
        let address = self.value(pointer)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = load atomic {llvm}, ptr {address} {}, align {align}",
            load_order(order)
        ));
        self.record(super::single(results)?, dest);
        Ok(())
    }

    fn atomic_store(
        &mut self,
        value: &Name,
        pointer: &Name,
        order: MemoryOrder,
    ) -> Result<(), EmitError> {
        let (llvm, align) = typed(&self.ty_of(value)?.clone())?;
        let (operand, address) = (self.value(value)?, self.value(pointer)?);
        self.line(&format!(
            "  store atomic {llvm} {operand}, ptr {address} {}, align {align}",
            store_order(order)
        ));
        Ok(())
    }

    fn atomic_rmw(
        &mut self,
        results: &[Name],
        operator: AtomicOperator,
        pointer: &Name,
        value: &Name,
        order: MemoryOrder,
    ) -> Result<(), EmitError> {
        let (llvm, align) = typed(&self.ty_of(value)?.clone())?;
        let (operand, address) = (self.value(value)?, self.value(pointer)?);
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = atomicrmw {} ptr {address}, {llvm} {operand} {}, align {align}",
            rmw_spelling(operator),
            spelling(order)
        ));
        self.record(super::single(results)?, dest);
        Ok(())
    }

    fn compare_exchange(
        &mut self,
        results: &[Name],
        weak: bool,
        pointer: &Name,
        expected: &Name,
        replacement: &Name,
        (success, failure): (MemoryOrder, MemoryOrder),
    ) -> Result<(), EmitError> {
        let (llvm, align) = typed(&self.ty_of(expected)?.clone())?;
        let address = self.value(pointer)?;
        let (wanted, new) = (self.value(expected)?, self.value(replacement)?);
        let pair = self.fresh();
        self.line(&format!(
            "  {pair} = cmpxchg{} ptr {address}, {llvm} {wanted}, {llvm} {new} {} {}, align {align}",
            if weak { " weak" } else { "" },
            spelling(success),
            failure_order(failure)
        ));
        let (previous, exchanged) = (self.fresh(), self.fresh());
        self.line(&format!(
            "  {previous} = extractvalue {{ {llvm}, i1 }} {pair}, 0"
        ));
        self.line(&format!(
            "  {exchanged} = extractvalue {{ {llvm}, i1 }} {pair}, 1"
        ));
        self.record_pair(results, previous, exchanged)
    }

    /// `asm "template", "constraints"(args)` (section 12.14) as a call to
    /// LLVM inline assembly with side effects. The spec gives `asm` no
    /// result type, so an `asm` that names a result is refused.
    fn inline_asm(&mut self, results: &[Name], asm: &klf_kir::InlineAsm) -> Result<(), EmitError> {
        if !results.is_empty() {
            return Err(EmitError(
                "`asm` has no result type to give its result (section 12.14)".to_owned(),
            ));
        }
        let mut operands = Vec::with_capacity(asm.arguments.len());
        for argument in &asm.arguments {
            let ty = llvm_type(&self.ty_of(argument)?.clone())?;
            operands.push(format!("{ty} {}", self.value(argument)?));
        }
        self.line(&format!(
            "  call void asm sideeffect \"{}\", \"{}\"({})",
            asm_escape(&asm.template),
            asm_escape(&asm.constraints),
            operands.join(", ")
        ));
        Ok(())
    }
}

impl FnEmitter<'_, '_> {
    fn set_jump(&mut self, results: &[Name], buffer: &Name) -> Result<(), EmitError> {
        self.intrinsic("declare i32 @setjmp(ptr) returns_twice".to_owned());
        let address = self.value(buffer)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call i32 @setjmp(ptr {address}) returns_twice"
        ));
        self.record(super::single(results)?, dest);
        Ok(())
    }

    fn long_jump(&mut self, buffer: &Name, value: &Name) -> Result<(), EmitError> {
        self.intrinsic("declare void @longjmp(ptr, i32) noreturn".to_owned());
        let (llvm, bits) = (
            llvm_type(&self.ty_of(value)?.clone())?,
            scalar_bits(&self.ty_of(value)?.clone())?,
        );
        let (address, operand) = (self.value(buffer)?, self.value(value)?);
        let answer = if bits == 32 {
            operand
        } else {
            let resized = self.fresh();
            let how = if bits > 32 { "trunc" } else { "sext" };
            self.line(&format!("  {resized} = {how} {llvm} {operand} to i32"));
            resized
        };
        self.line(&format!(
            "  call void @longjmp(ptr {address}, i32 {answer}) noreturn"
        ));
        Ok(())
    }
}

/// `text` as the inside of an LLVM string constant.
fn asm_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte == b'"' || byte == b'\\' || !(0x20..0x7f).contains(&byte) {
            let _ = write!(out, "\\{byte:02X}");
        } else {
            out.push(char::from(byte));
        }
    }
    out
}
