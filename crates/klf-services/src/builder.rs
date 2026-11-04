//! An ergonomic builder over the section-12 KIR model (`KirBuilder`, 8.11).
//!
//! [`FunctionBuilder`] tracks a current block, hands out fresh SSA value and
//! block names, and appends instructions; [`ModuleBuilder`] collects finished
//! functions into a [`Module`]. Value names it mints are prefixed so they
//! cannot collide with source identifiers.

mod operations;

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
    abi: klf_kir::FunctionAbi,
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
            abi: klf_kir::FunctionAbi::Platform,
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

    /// Sets the convention of a declaration explicitly.
    pub fn set_abi(&mut self, abi: klf_kir::FunctionAbi) {
        self.abi = abi;
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
}
