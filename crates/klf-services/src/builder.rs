//! An ergonomic builder over the section-12 KIR model (`KirBuilder`, 8.11).
//!
//! [`FunctionBuilder`] tracks a current block, hands out fresh SSA value and
//! block names, and appends instructions; [`ModuleBuilder`] collects finished
//! functions into a [`Module`]. Value names it mints are prefixed so they
//! cannot collide with source identifiers.

use klf_core::Name;
use klf_kir::{
    BinaryOperator, Block, BlockTarget, ConvertKind, Engine, FloatBinaryOperator, FloatPredicate,
    FloatUnaryOperator, Function, GenericParameter, Instruction, IntegerPredicate, Linkage,
    LoadMode, Module, Operation, Overflow, Parameter, StoreMode, Type,
};
use klf_source::Span;
use klf_target::Target;

pub struct ModuleBuilder {
    name: Name,
    language: String,
    target: Target,
    types: Vec<klf_kir::TypeDeclaration>,
    globals: Vec<klf_kir::GlobalDeclaration>,
    witnesses: Vec<klf_kir::WitnessDeclaration>,
    functions: Vec<Function>,
    attributes: Vec<klf_kir::ModuleAttribute>,
    /// The attributes already added, so a repeated one is found at once: a
    /// module carries one `klf.debug.name` per generated function.
    attribute_set: std::collections::HashSet<klf_kir::ModuleAttribute>,
}

impl ModuleBuilder {
    #[must_use]
    pub fn new(name: impl Into<Name>, language: impl Into<String>, target: Target) -> Self {
        Self {
            name: name.into(),
            language: language.into(),
            target,
            types: Vec::new(),
            globals: Vec::new(),
            witnesses: Vec::new(),
            functions: Vec::new(),
            attributes: Vec::new(),
            attribute_set: std::collections::HashSet::new(),
        }
    }

    /// Adds a module attribute; one already there is not repeated.
    pub fn add_attribute(&mut self, attribute: klf_kir::ModuleAttribute) {
        if self.attribute_set.insert(attribute.clone()) {
            self.attributes.push(attribute);
        }
    }

    pub fn add_type(&mut self, declaration: klf_kir::TypeDeclaration) {
        self.types.push(declaration);
    }

    pub fn add_global(&mut self, declaration: klf_kir::GlobalDeclaration) {
        self.globals.push(declaration);
    }

    pub fn add_witness(&mut self, declaration: klf_kir::WitnessDeclaration) {
        self.witnesses.push(declaration);
    }

    pub fn add_function(&mut self, function: Function) {
        self.functions.push(function);
    }

    /// Produces a raw-stage module. Ownership passes canonicalize it later.
    #[must_use]
    pub fn finish(self) -> Module {
        Module {
            types: self.types,
            globals: self.globals,
            witnesses: self.witnesses,
            functions: self.functions,
            attributes: self.attributes,
            ..Module::empty(self.name, self.language, self.target, klf_kir::Stage::Raw)
        }
    }
}

thread_local! {
    static VALUE_NAMES: std::cell::RefCell<Vec<Name>> = const { std::cell::RefCell::new(Vec::new()) };
    static BLOCK_NAMES: std::cell::RefCell<Vec<Name>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The name `prefix` followed by `number`. Every function numbers its values
/// and blocks from zero, so each name is made once and shared by all of them.
fn numbered(
    names: &'static std::thread::LocalKey<std::cell::RefCell<Vec<Name>>>,
    prefix: &str,
    number: u32,
) -> Name {
    let number = number as usize;
    names.with(|names| {
        let mut names = names.borrow_mut();
        while names.len() <= number {
            let next = names.len();
            names.push(Name::new(format!("{prefix}{next}")));
        }
        names[number].clone()
    })
}

pub struct FunctionBuilder {
    name: Name,
    linkage: Linkage,
    generics: Vec<Name>,
    parameters: Vec<Parameter>,
    result: Type,
    failure: Option<Type>,
    engine: Engine,
    foreign: bool,
    c_prototype: Option<klf_kir::CPrototype>,
    attributes: Vec<klf_kir::FunctionAttribute>,
    // Block `bbN` is at position N, so switching blocks is not a search.
    pub(crate) blocks: Vec<Block>,
    pub(crate) current: usize,
    value_counter: u32,
    block_counter: u32,
    span: Span,
    /// Where the code being built came from. Every instruction pushed while it
    /// is set carries it, which is what puts the function in the line table
    /// (section 16.3).
    location: Option<Box<klf_kir::DebugLocation>>,
    /// The location the instruction pushed last carries. An instruction
    /// carries one only where it changes, and at the start of a block: the
    /// instructions after it come from the same place, and a function's KIR
    /// stays small enough to cache (section 16.3).
    placed: Option<Box<klf_kir::DebugLocation>>,
}

impl FunctionBuilder {
    /// Starts a function. The entry block mirrors the parameters, so parameter
    /// names are usable as SSA values directly (matching the KIR text form).
    #[must_use]
    pub fn new(name: impl Into<Name>, params: &[(Name, Type)], result: Type, span: Span) -> Self {
        let parameters: Vec<Parameter> = params
            .iter()
            .map(|(name, ty)| Parameter {
                name: name.clone(),
                convention: None,
                context: false,
                ty: ty.clone(),
                span,
            })
            .collect();
        let entry = Block {
            name: Name::new("bb0"),
            parameters: parameters.clone(),
            instructions: Vec::new(),
            span,
        };
        Self {
            name: name.into(),
            linkage: Linkage::Module,
            generics: Vec::new(),
            parameters,
            result,
            failure: None,
            engine: Engine::Runtime,
            foreign: false,
            c_prototype: None,
            attributes: Vec::new(),
            blocks: vec![entry],
            current: 0,
            value_counter: 0,
            block_counter: 1,
            span,
            location: None,
            placed: None,
        }
    }

    /// Records where the code being built came from, so every instruction
    /// pushed from now on carries it in the line table (section 16.3). A
    /// definition lowering a language sets it as it walks the source.
    pub fn at(&mut self, location: Option<klf_kir::DebugLocation>) {
        self.location = location.map(Box::new);
    }

    /// The location instructions are currently placed at.
    #[must_use]
    pub fn location(&self) -> Option<&klf_kir::DebugLocation> {
        self.location.as_deref()
    }

    pub fn set_linkage(&mut self, linkage: Linkage) {
        self.linkage = linkage;
    }

    pub fn set_generics(&mut self, generics: Vec<Name>) {
        self.generics = generics;
    }

    pub fn set_engine(&mut self, engine: Engine) {
        self.engine = engine;
    }

    /// Marks the function as a body-less declaration. `finish` then drops the
    /// blocks, because a KIR function without an entry block is only valid when
    /// it is foreign.
    pub fn set_foreign(&mut self, foreign: bool) {
        self.foreign = foreign;
    }

    /// Gives the function a C signature (section 26.2): a foreign import's
    /// parameters as C reads them, or a function C calls.
    pub fn set_c_prototype(&mut self, prototype: klf_kir::CPrototype) {
        self.c_prototype = Some(prototype);
    }

    #[must_use]
    pub fn c_prototype(&self) -> Option<&klf_kir::CPrototype> {
        self.c_prototype.as_ref()
    }

    /// Adds `attribute` to the function the builder finishes.
    pub fn add_attribute(&mut self, attribute: klf_kir::FunctionAttribute) {
        if !self.attributes.contains(&attribute) {
            self.attributes.push(attribute);
        }
    }

    #[must_use]
    pub fn fresh_value(&mut self) -> Name {
        let name = numbered(&VALUE_NAMES, "t", self.value_counter);
        self.value_counter += 1;
        name
    }

    /// Creates a new block with the given parameters and returns its name.
    /// Does not switch to it.
    pub fn new_block(&mut self, params: &[(Name, Type)]) -> Name {
        let name = numbered(&BLOCK_NAMES, "bb", self.block_counter);
        self.block_counter += 1;
        let parameters = params
            .iter()
            .map(|(name, ty)| Parameter {
                name: name.clone(),
                convention: None,
                context: false,
                ty: ty.clone(),
                span: self.span,
            })
            .collect();
        self.blocks.push(Block {
            name: name.clone(),
            parameters,
            instructions: Vec::new(),
            span: self.span,
        });
        name
    }

    /// Switches the cursor to a previously created block.
    ///
    /// # Panics
    /// Panics if no block with that name exists.
    pub fn switch_to(&mut self, block: &Name) {
        let numbered = block
            .as_str()
            .strip_prefix("bb")
            .and_then(|number| number.parse::<usize>().ok())
            .filter(|&position| self.blocks.get(position).is_some_and(|b| &b.name == block));
        self.current = numbered.unwrap_or_else(|| {
            self.blocks
                .iter()
                .position(|b| &b.name == block)
                .expect("switch_to a block that exists")
        });
    }

    /// The block a function starts in, which carries its parameters.
    #[must_use]
    pub fn entry_block(&self) -> Name {
        self.blocks[0].name.clone()
    }

    #[must_use]
    pub fn current_block(&self) -> Name {
        self.blocks[self.current].name.clone()
    }

    /// The name the function is built under.
    #[must_use]
    pub fn name(&self) -> &Name {
        &self.name
    }

    /// Where the function is declared; an `unreachable` in it reports this.
    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }

    /// True when the current block already ends in a terminator.
    #[must_use]
    pub fn is_terminated(&self) -> bool {
        self.blocks[self.current]
            .instructions
            .last()
            .is_some_and(|i| i.operation.is_terminator())
    }

    pub(crate) fn push(&mut self, results: Vec<Name>, operation: Operation) {
        let terminator = operation.is_terminator();
        let instructions = &mut self.blocks[self.current].instructions;
        let debug_location = if self.location.is_some()
            && (instructions.is_empty() || self.location != self.placed)
        {
            self.placed.clone_from(&self.location);
            self.location.clone()
        } else {
            None
        };
        instructions.push(Instruction {
            results,
            operation,
            debug_location,
            span: self.span,
        });
        // A terminated block is complete, so the room it grew into goes back
        // now rather than when its module finishes.
        if terminator {
            instructions.shrink_to_fit();
        }
    }

    pub(crate) fn push_value(&mut self, operation: Operation) -> Name {
        let result = self.fresh_value();
        self.push(vec![result.clone()], operation);
        result
    }

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
            abi: klf_kir::FunctionAbi::Platform,
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
