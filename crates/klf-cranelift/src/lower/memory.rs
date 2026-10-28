//! Values in memory: copies and destroys of counted values, stack slots,
//! heap allocations, loads and stores.

use super::{FnLowerer, cl_type, pointee, single};
use crate::{EmitError, err};
use cranelift::codegen::ir::{MemFlagsData, StackSlotData, StackSlotKind};
use cranelift::prelude::{InstBuilder, types};
use cranelift_module::{FuncId, Module};
use klf_core::Name;
use klf_kir::Type;

impl FnLowerer<'_> {
    pub(super) fn copy(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let ty = self.ty_of(value)?.clone();
        let v = self.value(value)?;
        if matches!(ty, Type::Reference(_)) {
            self.call_void(self.runtime.retain, &[v]);
        }
        self.define(results, v)
    }

    /// `ref.isunique %r`: whether `%r` holds the only strong count, for
    /// copy-on-write. An address outside the heap is never unique.
    pub(super) fn is_unique(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let v = self.value(value)?;
        let unique = self.call_one(self.runtime.is_unique, &[v]);
        self.define(results, unique)
    }

    pub(super) fn destroy(&mut self, value: &Name) -> Result<(), EmitError> {
        let ty = self.ty_of(value)?.clone();
        let v = self.value(value)?;
        match ty {
            Type::Reference(_) => self.call_void(self.runtime.release, &[v]),
            Type::Unique(_) => self.call_void(self.runtime.free_raw, &[v]),
            _ => {}
        }
        Ok(())
    }

    /// A `stack.alloc` of a function whose addresses do not escape: a native
    /// stack slot, released with the frame. Definite initialization (section
    /// 11.6) stores to it before anything reads it, so it is not zeroed.
    pub(super) fn stack_slot(&mut self, results: &[Name], ty: &Type) -> Result<(), EmitError> {
        let layout = self.layouts.layout_of(ty);
        let align = layout.align.max(1).next_power_of_two();
        #[allow(clippy::cast_possible_truncation)]
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            layout.size.max(1) as u32,
            align.trailing_zeros() as u8,
        ));
        let result = single(results)?;
        self.slots.insert(result.clone(), slot);
        Ok(())
    }

    pub(super) fn alloc(
        &mut self,
        results: &[Name],
        ty: &Type,
        callee: FuncId,
    ) -> Result<(), EmitError> {
        let layout = self.layouts.layout_of(ty);
        #[allow(clippy::cast_possible_wrap)]
        let size = self.builder.ins().iconst(types::I64, layout.size as i64);
        #[allow(clippy::cast_possible_wrap)]
        let align = self
            .builder
            .ins()
            .iconst(types::I64, layout.align.max(1) as i64);
        let pointer = self.call_one(callee, &[size, align]);
        self.define(results, pointer)
    }

    pub(super) fn alloc_array(
        &mut self,
        results: &[Name],
        element: &Type,
        count: &Name,
    ) -> Result<(), EmitError> {
        let layout = self.layouts.layout_of(element);
        #[allow(clippy::cast_possible_wrap)]
        let element_size = self.builder.ins().iconst(types::I64, layout.size as i64);
        let count_value = self.value(count)?;
        let size = self.builder.ins().imul(element_size, count_value);
        #[allow(clippy::cast_possible_wrap)]
        let align = self
            .builder
            .ins()
            .iconst(types::I64, layout.align.max(1) as i64);
        let pointer = self.call_one(self.runtime.heap_alloc_untracked, &[size, align]);
        self.define(results, pointer)
    }

    /// A `load` is a machine load. Bounds checks belong to the hosted language
    /// (`KLF-RT-0005`); the VM stays the checked oracle.
    pub(super) fn load(&mut self, results: &[Name], address: &Name) -> Result<(), EmitError> {
        let result = single(results)?.clone();
        let result_ty = self.ty_of(&result)?.clone();
        let addr = self.value(address)?;
        let value = self
            .builder
            .ins()
            .load(cl_type(&result_ty)?, MemFlagsData::new(), addr, 0);
        self.values.insert(result, value);
        Ok(())
    }

    pub(super) fn store(&mut self, value: &Name, address: &Name) -> Result<(), EmitError> {
        let addr = self.value(address)?;
        let operand = self.value(value)?;
        self.builder
            .ins()
            .store(MemFlagsData::new(), operand, addr, 0);
        Ok(())
    }

    /// `field.addr`, `elem.addr` and `global.addr`: addresses computed from a
    /// base and a layout.
    pub(super) fn field_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        field: u32,
    ) -> Result<(), EmitError> {
        let offsets: std::rc::Rc<[u64]> = match self.types.get(base).and_then(pointee) {
            Some(Type::Nominal(name, _)) => self.layouts.field_offsets(name.as_str()),
            Some(Type::Tuple(elements)) => self.layouts.tuple_offsets(elements).into(),
            _ => return Err(err("`field.addr` base is not a struct or tuple address")),
        };
        let offset = *offsets
            .get(field as usize)
            .ok_or_else(|| err("`field.addr` field index out of range"))?;
        let base_value = self.value(base)?;
        #[allow(clippy::cast_possible_wrap)]
        let off = self.builder.ins().iconst(types::I64, offset as i64);
        let address = self.builder.ins().iadd(base_value, off);
        self.define(results, address)
    }

    pub(super) fn elem_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        index: &Name,
    ) -> Result<(), EmitError> {
        let element = match self.types.get(base).and_then(pointee) {
            Some(Type::Array(_, element) | Type::Vector(_, element)) => (**element).clone(),
            _ => return Err(err("`elem.addr` base is not an array address")),
        };
        let layout = self.layouts.layout_of(&element);
        let stride = layout.size.div_ceil(layout.align.max(1)) * layout.align.max(1);
        let base_value = self.value(base)?;
        let mut index_value = self.value(index)?;
        if self.builder.func.dfg.value_type(index_value) != types::I64 {
            index_value = self.builder.ins().sextend(types::I64, index_value);
        }
        #[allow(clippy::cast_possible_wrap)]
        let stride_value = self.builder.ins().iconst(types::I64, stride as i64);
        let offset = self.builder.ins().imul(index_value, stride_value);
        let address = self.builder.ins().iadd(base_value, offset);
        self.define(results, address)
    }

    pub(super) fn global_addr(&mut self, results: &[Name], global: &Name) -> Result<(), EmitError> {
        let data_id = *self
            .globals
            .get(global.as_str())
            .ok_or_else(|| err(format!("reference to unknown global `@{global}`")))?;
        let slot = self.module.declare_data_in_func(data_id, self.builder.func);
        if self.module.declarations().get_data_decl(data_id).tls {
            let address = self.builder.ins().tls_value(types::I64, slot);
            return self.define(results, address);
        }
        let slot_addr = self.builder.ins().symbol_value(types::I64, slot);
        let address = self
            .builder
            .ins()
            .load(types::I64, MemFlagsData::new(), slot_addr, 0);
        self.define(results, address)
    }
}
