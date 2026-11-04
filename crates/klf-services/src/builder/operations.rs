use super::*;

impl FunctionBuilder {
    #[must_use]
    pub fn const_int(&mut self, ty: Type, value: i128) -> Name {
        self.push_value(Operation::ConstInt { ty, value })
    }

    #[must_use]
    pub fn const_bool(&mut self, value: bool) -> Name {
        self.push_value(Operation::ConstBool(value))
    }

    #[must_use]
    pub fn const_float(&mut self, ty: Type, literal: String) -> Name {
        self.push_value(Operation::ConstFloat { ty, literal })
    }

    #[must_use]
    pub fn const_string(&mut self, value: String) -> (Name, Name) {
        let address = self.fresh_value();
        let length = self.fresh_value();
        self.push(
            vec![address.clone(), length.clone()],
            Operation::ConstString(value),
        );
        (address, length)
    }

    #[must_use]
    pub fn const_null(&mut self) -> Name {
        self.push_value(Operation::ConstNull)
    }

    #[must_use]
    pub fn heap_alloc_raw(&mut self, size: Name, align: Name) -> Name {
        self.push_value(Operation::HeapAllocRaw { size, align })
    }

    pub fn heap_free_raw(&mut self, pointer: Name) {
        self.push(Vec::new(), Operation::HeapFreeRaw(pointer));
    }

    pub fn mem_copy(&mut self, dest: Name, source: Name, length: Name) {
        self.push(
            Vec::new(),
            Operation::MemCopy {
                dest,
                source,
                length,
            },
        );
    }

    pub fn mem_move(&mut self, dest: Name, source: Name, length: Name) {
        self.push(
            Vec::new(),
            Operation::MemMove {
                dest,
                source,
                length,
            },
        );
    }

    pub fn mem_set(&mut self, dest: Name, byte: Name, length: Name) {
        self.push(Vec::new(), Operation::MemSet { dest, byte, length });
    }

    #[must_use]
    pub fn heap_alloc_array(&mut self, element: Type, count: Name) -> Name {
        self.push_value(Operation::HeapAllocArray { element, count })
    }

    #[must_use]
    pub fn const_zero(&mut self, ty: Type) -> Name {
        self.push_value(Operation::ConstZero(ty))
    }

    #[must_use]
    pub fn binary(
        &mut self,
        operator: BinaryOperator,
        overflow: Overflow,
        signed: bool,
        lhs: Name,
        rhs: Name,
    ) -> Name {
        self.push_value(Operation::Binary {
            operator,
            overflow,
            signed,
            lhs,
            rhs,
        })
    }

    #[must_use]
    pub fn compare(&mut self, predicate: IntegerPredicate, lhs: Name, rhs: Name) -> Name {
        self.push_value(Operation::IntegerCompare {
            predicate,
            lhs,
            rhs,
        })
    }

    #[must_use]
    pub fn float_binary(&mut self, operator: FloatBinaryOperator, lhs: Name, rhs: Name) -> Name {
        self.push_value(Operation::FloatBinary { operator, lhs, rhs })
    }

    #[must_use]
    pub fn float_unary(&mut self, operator: FloatUnaryOperator, value: Name) -> Name {
        self.push_value(Operation::FloatUnary { operator, value })
    }

    #[must_use]
    pub fn float_compare(&mut self, predicate: FloatPredicate, lhs: Name, rhs: Name) -> Name {
        self.push_value(Operation::FloatCompare {
            predicate,
            lhs,
            rhs,
        })
    }

    #[must_use]
    pub fn call(&mut self, callee: Name, arguments: Vec<Name>) -> Name {
        self.call_generic(callee, Vec::new(), arguments)
    }

    #[must_use]
    pub fn call_generic(
        &mut self,
        callee: Name,
        generic_args: Vec<Type>,
        arguments: Vec<Name>,
    ) -> Name {
        self.push_value(Operation::Call {
            callee,
            generic_args,
            arguments,
        })
    }

    pub fn call_void(&mut self, callee: Name, arguments: Vec<Name>) {
        self.call_generic_void(callee, Vec::new(), arguments);
    }

    pub fn call_generic_void(
        &mut self,
        callee: Name,
        generic_args: Vec<Type>,
        arguments: Vec<Name>,
    ) {
        self.push(
            Vec::new(),
            Operation::Call {
                callee,
                generic_args,
                arguments,
            },
        );
    }

    #[must_use]
    pub fn function_reference(&mut self, function: Name) -> Name {
        self.push_value(Operation::FunctionReference(function))
    }

    #[must_use]
    pub fn global_address(&mut self, global: Name) -> Name {
        self.push_value(Operation::GlobalAddress {
            global,
            thread_local: false,
        })
    }

    #[must_use]
    pub fn call_indirect(
        &mut self,
        target: Name,
        params: Vec<Type>,
        result: Type,
        arguments: Vec<Name>,
    ) -> Name {
        self.push_value(Operation::CallIndirect(Box::new(klf_kir::IndirectCall {
            target,
            params,
            result,
            arguments,
        })))
    }

    pub fn call_indirect_void(&mut self, target: Name, params: Vec<Type>, arguments: Vec<Name>) {
        self.push(
            Vec::new(),
            Operation::CallIndirect(Box::new(klf_kir::IndirectCall {
                target,
                params,
                result: Type::Unit,
                arguments,
            })),
        );
    }

    /// `call.closure %closure(arguments) : (params) -> result`.
    #[must_use]
    pub fn call_closure(
        &mut self,
        closure: Name,
        params: Vec<Type>,
        result: Type,
        arguments: Vec<Name>,
    ) -> Name {
        self.push_value(Operation::CallClosure(Box::new(klf_kir::ClosureCall {
            closure,
            params,
            result,
            arguments,
        })))
    }

    pub fn call_closure_void(&mut self, closure: Name, params: Vec<Type>, arguments: Vec<Name>) {
        self.push(
            Vec::new(),
            Operation::CallClosure(Box::new(klf_kir::ClosureCall {
                closure,
                params,
                result: Type::Unit,
                arguments,
            })),
        );
    }

    #[must_use]
    pub fn call_foreign(&mut self, symbol: Name, arguments: Vec<Name>) -> Name {
        self.push_value(Operation::CallForeign { symbol, arguments })
    }

    pub fn call_foreign_void(&mut self, symbol: Name, arguments: Vec<Name>) {
        self.push(Vec::new(), Operation::CallForeign { symbol, arguments });
    }

    #[must_use]
    pub fn convert(&mut self, kind: ConvertKind, value: Name, ty: Type) -> Name {
        self.push_value(Operation::Convert { kind, value, ty })
    }

    #[must_use]
    pub fn addr_cast(&mut self, address: Name, ty: Type) -> Name {
        self.push_value(Operation::AddrCast { address, ty })
    }

    #[must_use]
    pub fn field_addr(&mut self, base: Name, field: u32) -> Name {
        self.push_value(Operation::FieldAddr { base, field })
    }

    #[must_use]
    pub fn elem_addr(&mut self, base: Name, index: Name) -> Name {
        self.push_value(Operation::ElemAddr { base, index })
    }

    #[must_use]
    pub fn enum_tag(&mut self, value: Name) -> Name {
        self.push_value(Operation::EnumTag { value })
    }

    #[must_use]
    pub fn enum_payload_addr(&mut self, base: Name, variant: u32) -> Name {
        self.push_value(Operation::EnumPayloadAddr { base, variant })
    }

    pub fn enum_inject(&mut self, address: Name, variant: u32) {
        self.push(Vec::new(), Operation::EnumInject { address, variant });
    }

    #[must_use]
    pub fn stack_alloc(&mut self, ty: Type) -> Name {
        self.push_value(Operation::StackAlloc(ty))
    }

    pub fn stack_dealloc(&mut self, slot: Name) {
        self.push(Vec::new(), Operation::StackDealloc(slot));
    }

    #[must_use]
    pub fn load(&mut self, address: Name) -> Name {
        self.push_value(Operation::Load {
            mode: LoadMode::Trivial,
            address,
        })
    }

    /// `load [mode]`: `copy` yields a second owner, `take` moves the value out.
    #[must_use]
    pub fn load_with(&mut self, mode: LoadMode, address: Name) -> Name {
        self.push_value(Operation::Load { mode, address })
    }

    /// `destroy %value`: ends an owned value.
    pub fn destroy(&mut self, value: Name) {
        self.push(Vec::new(), Operation::Destroy(value));
    }

    /// `copy %value`: a second owner of `%value`; a `ref` gains a count.
    #[must_use]
    pub fn copy(&mut self, value: Name) -> Name {
        self.push_value(Operation::Copy(value))
    }

    /// `from.raw T, %pointer`: adopts `%pointer` as an owned `T`.
    #[must_use]
    pub fn from_raw(&mut self, ty: Type, pointer: Name) -> Name {
        self.push_value(Operation::FromRaw { ty, pointer })
    }

    /// `into.raw %value`: consumes an owned value without ending it.
    #[must_use]
    pub fn into_raw(&mut self, value: Name) -> Name {
        self.push_value(Operation::IntoRaw(value))
    }

    /// `ref.isunique %reference`: whether `%reference` holds the only count.
    #[must_use]
    pub fn ref_is_unique(&mut self, reference: Name) -> Name {
        self.push_value(Operation::RefIsUnique(reference))
    }

    pub fn store(&mut self, mode: StoreMode, value: Name, address: Name) {
        self.push(
            Vec::new(),
            Operation::Store {
                mode,
                value,
                address,
            },
        );
    }

    pub fn branch(&mut self, target: Name, arguments: Vec<Name>) {
        self.push(
            Vec::new(),
            Operation::Branch(BlockTarget {
                block: target,
                arguments,
            }),
        );
    }

    pub fn conditional_branch(
        &mut self,
        condition: Name,
        then_block: Name,
        then_args: Vec<Name>,
        else_block: Name,
        else_args: Vec<Name>,
    ) {
        self.push(
            Vec::new(),
            Operation::ConditionalBranch(Box::new(klf_kir::ConditionalBranch {
                condition,
                then_target: BlockTarget {
                    block: then_block,
                    arguments: then_args,
                },
                else_target: BlockTarget {
                    block: else_block,
                    arguments: else_args,
                },
            })),
        );
    }

    pub fn switch_enum(
        &mut self,
        value: Name,
        cases: Vec<(u32, BlockTarget)>,
        default: Option<BlockTarget>,
    ) {
        self.push(
            Vec::new(),
            Operation::SwitchEnum(Box::new(klf_kir::EnumSwitch {
                value,
                cases,
                default,
            })),
        );
    }

    pub fn switch_int(
        &mut self,
        value: Name,
        cases: Vec<(i128, BlockTarget)>,
        default: BlockTarget,
    ) {
        self.push(
            Vec::new(),
            Operation::SwitchInt(Box::new(klf_kir::IntegerSwitch {
                value,
                cases,
                default,
            })),
        );
    }

    pub fn ret(&mut self, value: Option<Name>) {
        self.push(Vec::new(), Operation::Return(value));
    }

    pub fn trap(&mut self, message: impl Into<String>) {
        self.push(Vec::new(), Operation::Trap(message.into()));
    }

    pub fn unreachable(&mut self) {
        self.push(Vec::new(), Operation::Unreachable);
    }

    /// Finishes the function. Any block left without a terminator (an
    /// unreachable join created during control-flow lowering) is closed with
    /// `unreachable`, so the result is always terminator-complete KIR.
    #[must_use]
    pub fn finish(mut self) -> Function {
        if self.foreign {
            self.blocks.clear();
        }
        for block in &mut self.blocks {
            let terminated = block
                .instructions
                .last()
                .is_some_and(|i| i.operation.is_terminator());
            if !terminated {
                block.instructions.push(Instruction {
                    results: Vec::new(),
                    operation: Operation::Unreachable,
                    debug_location: self.location.clone(),
                    span: self.span,
                });
            }
            // A module keeps every function it builds, so the room a block
            // grew into goes back rather than staying with it.
            block.instructions.shrink_to_fit();
        }
        self.blocks.shrink_to_fit();
        Function {
            name: self.name,
            linkage: self.linkage,
            abi: self.abi,
            nounwind: true,
            foreign: self.foreign,
            c_prototype: self.c_prototype,
            generics: self
                .generics
                .into_iter()
                .map(GenericParameter::unbounded)
                .collect(),
            attributes: self.attributes.clone(),
            parameters: self.parameters,
            result: self.result,
            failure: self.failure,
            engine: self.engine,
            blocks: self.blocks,
            span: self.span,
        }
    }
}
