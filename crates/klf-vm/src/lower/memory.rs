//! Lowering of address arithmetic, allocation and memory access.

use super::{LowerError, Lowerer, err, scalar_width};
use crate::bytecode::{Instr, Opcode, OverflowMode, WidthCode, arith_flags};
use crate::program::AllocShape;
use klf_core::Name;
use klf_kir::{Operation, Type};

impl Lowerer<'_> {
    pub(super) fn lower_memory(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        if let Some(result) = self.lower_promoted_memory(results, operation) {
            return Some(result);
        }
        let result = match operation {
            Operation::Copy(value) => self.lower_copy(results, value),
            Operation::Destroy(value) => self.lower_destroy(value),
            Operation::RefIsUnique(value) => self.register(value).and_then(|source| {
                let dest = self.result(results)?;
                self.emit(Instr::abc(Opcode::IsUnique, dest, source, 0));
                Ok(())
            }),
            Operation::FieldAddr { base, field } => self.lower_field_addr(results, base, *field),
            Operation::ElemAddr { base, index } => self.lower_elem_addr(results, base, index),
            Operation::GlobalAddress { global, .. } => self.lower_global_address(results, global),
            Operation::EnumTag { value } => self.lower_enum_tag(results, value),
            Operation::EnumPayloadAddr { base, variant } => {
                self.lower_enum_payload_addr(results, base, *variant)
            }
            Operation::EnumInject { address, variant } => self.lower_enum_inject(address, *variant),
            Operation::StackAlloc(ty) => self.lower_alloc(results, Opcode::Alloca, ty),
            Operation::HeapAllocCounted(ty) => self.lower_alloc(results, Opcode::HallocC, ty),
            Operation::HeapAllocUnique(ty) => self.lower_alloc(results, Opcode::HallocU, ty),
            Operation::HeapAllocArray { element, count } => {
                self.lower_alloc_array(results, element, count)
            }
            Operation::HeapAllocRaw { size, align } => self.lower_alloc_raw(results, size, align),
            Operation::HeapFreeRaw(pointer) => self.register(pointer).map(|register| {
                self.emit(Instr::abc(Opcode::HfreeR, register, 0, 0));
            }),
            Operation::MemCopy {
                dest,
                source,
                length,
            } => self.lower_bulk(Opcode::MemCpy, dest, source, length),
            Operation::MemMove {
                dest,
                source,
                length,
            } => self.lower_bulk(Opcode::MemMove, dest, source, length),
            Operation::MemSet { dest, byte, length } => {
                self.lower_bulk(Opcode::MemSet, dest, byte, length)
            }
            Operation::StackDealloc(address) => {
                let register = match self.register(address) {
                    Ok(register) => register,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::abc(Opcode::HfreeR, register, 0, 0));
                Ok(())
            }
            Operation::Load { address, .. } => self.lower_load(results, address),
            Operation::Store { value, address, .. } => self.lower_store(value, address),
            Operation::BorrowBegin(_) | Operation::BorrowEnd(_) => Ok(()),
            _ => return None,
        };
        Some(result)
    }

    fn lower_global_address(&mut self, results: &[Name], global: &Name) -> Result<(), LowerError> {
        let index = *self
            .globals
            .get(global.as_str())
            .ok_or_else(|| err(format!("reference to unknown global `@{global}`")))?;
        let dest = self.result(results)?;
        let index = i32::try_from(index).map_err(|_| err("too many globals"))?;
        self.emit(Instr::from_bx(Opcode::GlobalA, dest, index));
        Ok(())
    }

    pub(super) fn pointee_of(&self, base: &Name) -> Option<Type> {
        match self.types.get(base) {
            Some(Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner)) => {
                Some((**inner).clone())
            }
            _ => None,
        }
    }

    fn lower_field_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        field: u32,
    ) -> Result<(), LowerError> {
        let offsets = match self.pointee_of(base) {
            Some(Type::Nominal(name, _)) => klf_layout::field_offsets(name.as_str(), self.decls),
            Some(Type::Tuple(elements)) => klf_layout::aggregate(&elements, self.decls).1,
            _ => return Err(err("`field.addr` base is not a struct or tuple address")),
        };
        let offset = *offsets
            .get(field as usize)
            .ok_or_else(|| err("`field.addr` field index out of range"))?;
        let dest = self.result(results)?;
        let base_reg = self.register(base)?;
        let off_reg = self.load_u64_const(offset)?;
        self.emit(
            Instr::abc(Opcode::Add, dest, base_reg, off_reg).with_flags(arith_flags(
                WidthCode::W64,
                false,
                OverflowMode::Wrap,
            )),
        );
        Ok(())
    }

    fn lower_bulk(
        &mut self,
        opcode: Opcode,
        dest: &Name,
        source: &Name,
        length: &Name,
    ) -> Result<(), LowerError> {
        let (dest, source, length) = (
            self.register(dest)?,
            self.register(source)?,
            self.register(length)?,
        );
        self.emit(Instr::abc(opcode, dest, source, length));
        Ok(())
    }

    fn lower_alloc_raw(
        &mut self,
        results: &[Name],
        size: &Name,
        align: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let size = self.register(size)?;
        let align = self.register(align)?;
        self.emit(Instr::abc(Opcode::HallocR, dest, size, align));
        Ok(())
    }

    fn lower_alloc_array(
        &mut self,
        results: &[Name],
        element: &Type,
        count: &Name,
    ) -> Result<(), LowerError> {
        let layout = klf_layout::layout_of(element, self.decls);
        let stride = layout.size.div_ceil(layout.align.max(1)) * layout.align.max(1);
        let dest = self.result(results)?;
        let count_reg = self.register(count)?;
        let stride_reg = self.load_u64_const(stride)?;
        let align_reg = self.load_u64_const(layout.align.max(1))?;
        let size_reg = self.fresh()?;
        let flags = arith_flags(WidthCode::W64, false, OverflowMode::Wrap);
        self.emit(Instr::abc(Opcode::Mul, size_reg, count_reg, stride_reg).with_flags(flags));
        self.emit(Instr::abc(Opcode::HallocR, dest, size_reg, align_reg));
        Ok(())
    }

    fn lower_elem_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        index: &Name,
    ) -> Result<(), LowerError> {
        let element = match self.pointee_of(base) {
            Some(Type::Array(_, element) | Type::Vector(_, element)) => *element,
            _ => return Err(err("`elem.addr` base is not an array address")),
        };
        let layout = klf_layout::layout_of(&element, self.decls);
        let stride = layout.size.div_ceil(layout.align.max(1)) * layout.align.max(1);
        let dest = self.result(results)?;
        let base_reg = self.register(base)?;
        let index_reg = self.register(index)?;
        let stride_reg = self.load_u64_const(stride)?;
        let offset = self.fresh()?;
        let flags = arith_flags(WidthCode::W64, false, OverflowMode::Wrap);
        self.emit(Instr::abc(Opcode::Mul, offset, index_reg, stride_reg).with_flags(flags));
        self.emit(Instr::abc(Opcode::Add, dest, base_reg, offset).with_flags(flags));
        Ok(())
    }
    fn lower_copy(&mut self, results: &[Name], value: &Name) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let source = self.register(value)?;
        self.emit(Instr::abc(crate::bytecode::Opcode::Mov, dest, source, 0));
        if matches!(self.types.get(value), Some(Type::Reference(_))) {
            self.emit(Instr::abc(crate::bytecode::Opcode::Retain, dest, 0, 0));
        }
        Ok(())
    }

    fn lower_destroy(&mut self, value: &Name) -> Result<(), LowerError> {
        let register = self.register(value)?;
        match self.types.get(value) {
            Some(Type::Reference(_)) => {
                self.emit(Instr::abc(crate::bytecode::Opcode::Release, register, 0, 0));
            }
            Some(Type::Unique(_)) => {
                self.emit(Instr::abc(crate::bytecode::Opcode::HfreeR, register, 0, 0));
            }
            _ => {}
        }
        Ok(())
    }

    fn lower_alloc(
        &mut self,
        results: &[Name],
        opcode: crate::bytecode::Opcode,
        ty: &Type,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let layout = klf_layout::layout_of(ty, self.decls);
        let size = u32::try_from(layout.size).map_err(|_| err("allocation too large"))?;
        let align = u32::try_from(layout.align.max(1)).map_err(|_| err("alignment too large"))?;
        let index = u16::try_from(self.allocations.len())
            .map_err(|_| err("too many allocations in one function"))?;
        self.allocations.push(AllocShape { size, align });
        if self.trace
            && opcode == crate::bytecode::Opcode::HallocC
            && let Some(map) = klf_layout::trace_map(ty, self.decls)
        {
            self.traces.push((index, map.into()));
        }
        self.emit(Instr::from_bx(opcode, dest, i32::from(index)));
        Ok(())
    }

    pub(super) fn lower_load(&mut self, results: &[Name], address: &Name) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let source = self.register(address)?;
        let width = match results {
            [single] => scalar_width(self.type_of(single)?)?,
            _ => return Err(err("`load` needs exactly one result")),
        };
        self.emit(
            Instr::abc(crate::bytecode::Opcode::Load, dest, source, 0).with_flags(width.code()),
        );
        Ok(())
    }

    pub(super) fn lower_store(&mut self, value: &Name, address: &Name) -> Result<(), LowerError> {
        let address_register = self.register(address)?;
        let value_register = self.register(value)?;
        let width = scalar_width(self.type_of(value)?)?;
        self.emit(
            Instr::abc(
                crate::bytecode::Opcode::Store,
                address_register,
                value_register,
                0,
            )
            .with_flags(width.code()),
        );
        Ok(())
    }

    fn type_of(&self, name: &Name) -> Result<&Type, LowerError> {
        self.types
            .get(name)
            .ok_or_else(|| err(format!("cannot type `%{name}`")))
    }
}
