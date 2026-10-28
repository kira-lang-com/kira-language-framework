//! The builder methods for the section-12 instructions close to the machine:
//! `select` and `fma`, the raw pointer instructions of 12.8, the atomics and
//! `fence` (12.14), the layout queries of 12.16, and the `vec.*`
//! instructions of 12.5.

use crate::FunctionBuilder;
use klf_core::Name;
use klf_kir::{AtomicOperator, MemoryOrder, Operation, RawAccess, Type};

impl FunctionBuilder {
    #[must_use]
    pub fn select(&mut self, condition: Name, then_value: Name, else_value: Name) -> Name {
        self.push_value(Operation::Select {
            condition,
            then_value,
            else_value,
        })
    }

    #[must_use]
    pub fn fma(&mut self, first: Name, second: Name, addend: Name) -> Name {
        self.push_value(Operation::FloatFma {
            first,
            second,
            addend,
        })
    }

    #[must_use]
    pub fn raw_load(&mut self, ty: Type, pointer: Name, access: RawAccess) -> Name {
        self.push_value(Operation::RawLoad {
            ty,
            pointer,
            access,
        })
    }

    pub fn raw_store(&mut self, value: Name, pointer: Name, access: RawAccess) {
        self.push(
            Vec::new(),
            Operation::RawStore {
                value,
                pointer,
                access,
            },
        );
    }

    #[must_use]
    pub fn pointer_offset(&mut self, pointer: Name, bytes: Name) -> Name {
        self.push_value(Operation::PointerOffset { pointer, bytes })
    }

    #[must_use]
    pub fn pointer_diff(&mut self, lhs: Name, rhs: Name) -> Name {
        self.push_value(Operation::PointerDiff { lhs, rhs })
    }

    #[must_use]
    pub fn atomic_load(&mut self, ty: Type, pointer: Name, order: MemoryOrder) -> Name {
        self.push_value(Operation::AtomicLoad { ty, pointer, order })
    }

    pub fn atomic_store(&mut self, value: Name, pointer: Name, order: MemoryOrder) {
        self.push(
            Vec::new(),
            Operation::AtomicStore {
                value,
                pointer,
                order,
            },
        );
    }

    #[must_use]
    pub fn atomic_rmw(
        &mut self,
        operator: AtomicOperator,
        pointer: Name,
        value: Name,
        order: MemoryOrder,
    ) -> Name {
        self.push_value(Operation::AtomicRmw {
            operator,
            pointer,
            value,
            order,
        })
    }

    /// The value read and whether the exchange succeeded.
    #[must_use]
    pub fn compare_exchange(
        &mut self,
        weak: bool,
        pointer: Name,
        expected: Name,
        replacement: Name,
        orders: (MemoryOrder, MemoryOrder),
    ) -> (Name, Name) {
        let read = self.fresh_value();
        let succeeded = self.fresh_value();
        self.push(
            vec![read.clone(), succeeded.clone()],
            Operation::AtomicCompareExchange {
                weak,
                pointer,
                expected,
                replacement,
                success: orders.0,
                failure: orders.1,
            },
        );
        (read, succeeded)
    }

    pub fn fence(&mut self, order: MemoryOrder) {
        self.push(Vec::new(), Operation::Fence(order));
    }

    #[must_use]
    pub fn type_size(&mut self, ty: Type) -> Name {
        self.push_value(Operation::TypeSize(ty))
    }

    #[must_use]
    pub fn type_align(&mut self, ty: Type) -> Name {
        self.push_value(Operation::TypeAlign(ty))
    }

    #[must_use]
    pub fn type_stride(&mut self, ty: Type) -> Name {
        self.push_value(Operation::TypeStride(ty))
    }

    #[must_use]
    pub fn vector_splat(&mut self, ty: Type, value: Name) -> Name {
        self.push_value(Operation::VectorSplat { ty, value })
    }

    #[must_use]
    pub fn vector_extract(&mut self, vector: Name, lane: u32) -> Name {
        self.push_value(Operation::VectorExtract { vector, lane })
    }

    #[must_use]
    pub fn vector_insert(&mut self, vector: Name, lane: u32, value: Name) -> Name {
        self.push_value(Operation::VectorInsert {
            vector,
            lane,
            value,
        })
    }

    #[must_use]
    pub fn vector_shuffle(&mut self, lhs: Name, rhs: Name, mask: Vec<u32>) -> Name {
        self.push_value(Operation::VectorShuffle { lhs, rhs, mask })
    }

    /// Adds `lane` to the mask of `shuffle`, which must be the instruction
    /// just built, so nothing has used its result yet. Answers whether it
    /// was.
    pub fn extend_shuffle(&mut self, shuffle: &Name, lane: u32) -> bool {
        let Some(last) = self.blocks[self.current].instructions.last_mut() else {
            return false;
        };
        match &mut last.operation {
            Operation::VectorShuffle { mask, .. }
                if last.results.as_slice() == [shuffle.clone()] =>
            {
                mask.push(lane);
                true
            }
            _ => false,
        }
    }
}
