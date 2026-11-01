//! `select`, `fma`, the raw pointer instructions (sections 12.7 and 12.8)
//! and the layout queries of section 12.16.

use super::FnEmitter;
use crate::EmitError;
use crate::lltype::llvm_type;
use klf_core::Name;
use klf_kir::{Operation, RawAccess};

impl FnEmitter<'_, '_> {
    pub(super) fn unmanaged(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), EmitError>> {
        Some(match operation {
            Operation::Select {
                condition,
                then_value,
                else_value,
            } => self.select(results, condition, then_value, else_value),
            Operation::FloatFma {
                first,
                second,
                addend,
            } => self.fma(results, [first, second, addend]),
            Operation::RawLoad {
                pointer, access, ..
            } => self.raw_load(results, pointer, *access),
            Operation::LoadBorrow(pointer) => self.load(results, pointer),
            Operation::RawStore {
                value,
                pointer,
                access,
            } => self.raw_store(value, pointer, *access),
            Operation::AddrToPointer(value) | Operation::RefUnowned(value) => {
                self.forward(results, value)
            }
            Operation::PointerOffset { pointer, bytes } => {
                self.pointer_offset(results, pointer, bytes)
            }
            Operation::PointerDiff { lhs, rhs } => self.pointer_diff(results, lhs, rhs),
            Operation::TypeSize(ty) | Operation::TypeAlign(ty) | Operation::TypeStride(ty) => {
                let layout = klf_layout::layout_of(ty, &self.module.types);
                let answer = match operation {
                    Operation::TypeSize(_) => layout.size,
                    Operation::TypeAlign(_) => layout.align,
                    _ if layout.align == 0 => layout.size,
                    _ => layout.size.div_ceil(layout.align) * layout.align,
                };
                self.const_int(results, &klf_kir::Type::Integer(64), i128::from(answer))
            }
            _ => return None,
        })
    }

    /// Unmanaged, explicitly qualified accesses must preserve the KIR flags.
    /// Ordinary accesses use the existing bounds-checked runtime helper.
    fn raw_load(
        &mut self,
        results: &[Name],
        pointer: &Name,
        access: RawAccess,
    ) -> Result<(), EmitError> {
        if access == RawAccess::default() {
            return self.load(results, pointer);
        }
        let result = super::single(results)?.clone();
        let ty = llvm_type(self.ty_of(&result)?)?;
        let address = self.value(pointer)?;
        let value = self.fresh();
        let qualifier = if access.volatile { "volatile " } else { "" };
        let align = access.align.unwrap_or(1);
        if align == 0 || !align.is_power_of_two() {
            return Err(EmitError(
                "raw load alignment must be a power of two".into(),
            ));
        }
        self.line(&format!(
            "  {value} = load {qualifier}{ty}, ptr {address}, align {align}"
        ));
        self.record(&result, value);
        Ok(())
    }

    fn raw_store(
        &mut self,
        value: &Name,
        pointer: &Name,
        access: RawAccess,
    ) -> Result<(), EmitError> {
        if access == RawAccess::default() {
            return self.store(value, pointer);
        }
        let ty = llvm_type(self.ty_of(value)?)?;
        let operand = self.value(value)?;
        let address = self.value(pointer)?;
        let qualifier = if access.volatile { "volatile " } else { "" };
        let align = access.align.unwrap_or(1);
        if align == 0 || !align.is_power_of_two() {
            return Err(EmitError(
                "raw store alignment must be a power of two".into(),
            ));
        }
        self.line(&format!(
            "  store {qualifier}{ty} {operand}, ptr {address}, align {align}"
        ));
        Ok(())
    }

    fn select(
        &mut self,
        results: &[Name],
        condition: &Name,
        then_value: &Name,
        else_value: &Name,
    ) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let condition_ty = llvm_type(self.ty_of(condition)?)?;
        let ty = llvm_type(self.ty_of(then_value)?)?;
        let condition = self.value(condition)?;
        let then_value = self.value(then_value)?;
        let else_value = self.value(else_value)?;
        let bit = if condition_ty == "i1" {
            condition
        } else {
            let bit = self.fresh();
            self.line(&format!("  {bit} = trunc {condition_ty} {condition} to i1"));
            bit
        };
        let name = self.fresh();
        self.line(&format!(
            "  {name} = select i1 {bit}, {ty} {then_value}, {ty} {else_value}"
        ));
        self.record(&result, name);
        Ok(())
    }

    fn fma(&mut self, results: &[Name], operands: [&Name; 3]) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let ty = llvm_type(self.ty_of(operands[0])?)?;
        let suffix = if ty == "float" { "f32" } else { "f64" };
        self.intrinsic(format!("declare {ty} @llvm.fma.{suffix}({ty}, {ty}, {ty})"));
        let [first, second, addend] = [
            self.value(operands[0])?,
            self.value(operands[1])?,
            self.value(operands[2])?,
        ];
        let name = self.fresh();
        self.line(&format!(
            "  {name} = call {ty} @llvm.fma.{suffix}({ty} {first}, {ty} {second}, {ty} {addend})"
        ));
        self.record(&result, name);
        Ok(())
    }

    fn forward(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let operand = self.value(value)?;
        self.record(&result, operand);
        Ok(())
    }

    fn pointer_offset(
        &mut self,
        results: &[Name],
        pointer: &Name,
        bytes: &Name,
    ) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let bytes_ty = llvm_type(self.ty_of(bytes)?)?;
        let pointer = self.value(pointer)?;
        let bytes = self.value(bytes)?;
        let name = self.fresh();
        self.line(&format!(
            "  {name} = getelementptr i8, ptr {pointer}, {bytes_ty} {bytes}"
        ));
        self.record(&result, name);
        Ok(())
    }

    fn pointer_diff(&mut self, results: &[Name], lhs: &Name, rhs: &Name) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let lhs = self.value(lhs)?;
        let rhs = self.value(rhs)?;
        let (left, right, name) = (self.fresh(), self.fresh(), self.fresh());
        self.line(&format!("  {left} = ptrtoint ptr {lhs} to i64"));
        self.line(&format!("  {right} = ptrtoint ptr {rhs} to i64"));
        self.line(&format!("  {name} = sub i64 {left}, {right}"));
        self.record(&result, name);
        Ok(())
    }
}
