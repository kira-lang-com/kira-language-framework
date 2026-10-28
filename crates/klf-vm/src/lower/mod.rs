//! Lowering of verified KIR functions to bytecode (section 15).

mod access;
mod atomic;
mod blocks;
mod calls;
mod constants;
mod control;
mod enums;
mod expr;
mod fallible;
mod foreign;
mod inline;

pub use foreign::{entered_from_c, entry_signature, foreign_class, vm_entry_symbol};
mod memory;
mod ownership;
mod promote;
mod promote_bytecode;
mod registers;
mod simplify;
mod unmanaged;

use crate::bytecode::{Instr, Opcode, WidthCode};
use crate::program::{
    AllocShape, BcFunction, BranchCondition, BranchSite, BranchTarget, CallSite, Const, Import,
    Program, RegisterMove, Traces,
};
use crate::types::{ValueTypes, integer_width};
use klf_core::Name;
use klf_kir::{
    Block, BlockTarget, FloatPredicate, Function, IntegerPredicate, Module, Operation, Type,
};
use std::collections::HashMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LowerError(pub String);

impl std::fmt::Display for LowerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn err(message: impl Into<String>) -> LowerError {
    LowerError(message.into())
}

/// Lower a whole module. Only `runtime` and `any` functions are compiled here.
///
/// # Errors
/// Returns [`LowerError`] when a function uses an operation or width the
/// bytecode backend does not yet support.
pub fn lower_module(module: &Module) -> Result<Program, LowerError> {
    lower(module, false).map(|(program, _)| program)
}

/// Lowers `module` as a debug build: each counted allocation of a type with
/// strong references also gets its trace map (section 18.6), for
/// [`crate::run_traced`].
///
/// # Errors
/// As [`lower_module`].
pub fn lower_module_traced(module: &Module) -> Result<(Program, Traces), LowerError> {
    lower(module, true)
}

fn lower(module: &Module, trace: bool) -> Result<(Program, Traces), LowerError> {
    let legal = klf_kir::legalize(module);
    let module = legal.as_ref().unwrap_or(module);
    let rewritten = foreign_calls(module);
    let module = rewritten.as_ref().unwrap_or(module);
    let mut consts = ConstPool::default();
    let mut functions = Vec::with_capacity(module.functions.len());
    let indices: HashMap<&str, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.as_str(), index))
        .collect();
    let results: HashMap<&str, Type> = module
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function.result.clone()))
        .collect();
    let global_indices: HashMap<&str, usize> = module
        .globals
        .iter()
        .enumerate()
        .map(|(index, global)| (global.name.as_str(), index))
        .collect();
    let global_types: HashMap<&str, Type> = module
        .globals
        .iter()
        .map(|global| (global.name.as_str(), global.ty.clone()))
        .collect();
    let mut globals = module
        .globals
        .iter()
        .map(|global| {
            let layout = klf_layout::layout_of(&global.ty, &module.types);
            let size = u32::try_from(layout.size.max(1)).unwrap_or(u32::MAX);
            let init = global.init.init_bytes(size as usize);
            crate::program::GlobalSpec {
                size,
                align: u32::try_from(layout.align.max(1)).unwrap_or(1),
                init,
            }
        })
        .collect::<Vec<_>>();
    let mut string_globals = HashMap::new();
    for instruction in module
        .functions
        .iter()
        .flat_map(|function| function.blocks.iter())
        .flat_map(|block| block.instructions.iter())
    {
        let Operation::ConstString(value) = &instruction.operation else {
            continue;
        };
        if string_globals.contains_key(value) {
            continue;
        }
        let index = globals.len();
        string_globals.insert(value.clone(), index);
        globals.push(crate::program::GlobalSpec {
            size: u32::try_from(value.len().max(1)).unwrap_or(u32::MAX),
            align: 1,
            init: value.as_bytes().to_vec(),
        });
    }
    let declared: HashMap<&str, &Function> = module
        .functions
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect();
    let tables = ModuleTables {
        functions: &declared,
        indices: &indices,
        results: &results,
        globals: &global_indices,
        global_types: &global_types,
        string_globals: &string_globals,
        decls: &module.types,
        trace,
    };
    let mut initializers = Vec::new();
    let mut traces = Traces::new();
    for function in &module.functions {
        if function
            .attributes
            .contains(&klf_kir::FunctionAttribute::Initializer)
            && !function.foreign
        {
            initializers.push(functions.len());
        }
        let (lowered, maps) = lower_function(function, &tables, &mut consts)?;
        let index = functions.len();
        traces.extend(maps.into_iter().map(|(shape, map)| ((index, shape), map)));
        functions.push(lowered);
    }
    for function in &mut functions {
        simplify::fallthroughs(function);
        simplify::local_registers(function);
    }
    for _ in 0..3 {
        inline::leaves(&mut functions, module);
        for function in &mut functions {
            constants::propagate(function);
            simplify::local_registers(function);
            promote_bytecode::scalar_slots(function);
            simplify::local_registers(function);
        }
    }
    let program = Program {
        functions,
        consts: consts.values,
        imports: consts.imports,
        globals,
        initializers,
    };
    Ok((program, traces))
}

/// A `call` to a foreign declaration is a platform C call, as the native
/// backends treat it; the VM has no body to enter. Returns the module with
/// those calls spelled `call.foreign`, or `None` when there are none.
fn foreign_calls(module: &Module) -> Option<Module> {
    let foreign: std::collections::HashSet<&str> = module
        .functions
        .iter()
        .filter(|function| function.foreign)
        .map(|function| function.name.as_str())
        .collect();
    let calls_foreign = |operation: &Operation| matches!(operation, Operation::Call { callee, .. } if foreign.contains(callee.as_str()));
    let any = module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
        .any(|instruction| calls_foreign(&instruction.operation));
    if !any {
        return None;
    }
    let mut rewritten = module.clone();
    for instruction in rewritten
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .flat_map(|block| &mut block.instructions)
    {
        if let Operation::Call {
            callee, arguments, ..
        } = &instruction.operation
            && foreign.contains(callee.as_str())
        {
            instruction.operation = Operation::CallForeign {
                symbol: callee.clone(),
                arguments: arguments.clone(),
            };
        }
    }
    Some(rewritten)
}

#[derive(Default)]
struct ConstPool {
    values: Vec<Const>,
    index: HashMap<Const, usize>,
    imports: Vec<Import>,
    import_index: HashMap<Import, usize>,
}

struct Lowerer<'a> {
    functions: &'a HashMap<&'a str, &'a Function>,
    registers: HashMap<Name, u16>,
    value_registers: u32,
    next_scratch: u32,
    register_count: u32,
    block_offset: HashMap<Name, usize>,
    types: ValueTypes,
    indices: &'a HashMap<&'a str, usize>,
    globals: &'a HashMap<&'a str, usize>,
    string_globals: &'a HashMap<String, usize>,
    decls: &'a [klf_kir::TypeDeclaration],
    consts: &'a mut ConstPool,
    code: Vec<Instr>,
    call_sites: Vec<CallSite>,
    allocations: Vec<AllocShape>,
    /// With `trace`, the trace map of each counted allocation shape that has
    /// one.
    trace: bool,
    traces: Vec<(u16, Box<[u8]>)>,
    promoted_slots: HashMap<Name, promote::Slot>,
    branches: Vec<BranchSite>,
    branch_fixups: Vec<BranchFixup>,
}

struct BranchFixup {
    site: usize,
    then_block: Name,
    else_block: Option<Name>,
}

struct ModuleTables<'a> {
    functions: &'a HashMap<&'a str, &'a Function>,
    indices: &'a HashMap<&'a str, usize>,
    results: &'a HashMap<&'a str, Type>,
    globals: &'a HashMap<&'a str, usize>,
    global_types: &'a HashMap<&'a str, Type>,
    string_globals: &'a HashMap<String, usize>,
    decls: &'a [klf_kir::TypeDeclaration],
    trace: bool,
}

type TracedFunction = (BcFunction, Vec<(u16, Box<[u8]>)>);

fn lower_function<'a>(
    function: &Function,
    tables: &ModuleTables<'a>,
    consts: &'a mut ConstPool,
) -> Result<TracedFunction, LowerError> {
    let types = ValueTypes::infer(function, tables.results, tables.global_types, tables.decls);
    let assignment = registers::assign(function)?;
    let (promoted_slots, persistent_count) = promote::assign(function, assignment.count)?;
    let mut lowerer = Lowerer {
        functions: tables.functions,
        registers: assignment.registers,
        value_registers: persistent_count,
        next_scratch: persistent_count,
        register_count: persistent_count,
        block_offset: HashMap::new(),
        types,
        indices: tables.indices,
        globals: tables.globals,
        string_globals: tables.string_globals,
        decls: tables.decls,
        consts,
        code: Vec::new(),
        call_sites: Vec::new(),
        allocations: Vec::new(),
        trace: tables.trace,
        traces: Vec::new(),
        promoted_slots,
        branches: Vec::new(),
        branch_fixups: Vec::new(),
    };
    let param_count = u32::try_from(function.parameters.len())
        .map_err(|_| err("function has too many parameters"))?;
    for block in &function.blocks {
        lowerer
            .block_offset
            .insert(block.name.clone(), lowerer.code.len());
        lowerer.lower_block(block)?;
    }
    lowerer.resolve_fixups()?;
    let code = lowerer.code.iter().map(|instr| instr.encode()).collect();
    let bytecode = BcFunction {
        name: function.name.clone(),
        register_count: lowerer.register_count,
        param_count,
        returns_value: !matches!(function.result, Type::Unit | Type::Never),
        code,
        call_sites: lowerer.call_sites,
        allocations: lowerer.allocations,
        branches: lowerer.branches,
    };
    Ok((bytecode, lowerer.traces))
}

impl Lowerer<'_> {
    fn register(&self, name: &Name) -> Result<u16, LowerError> {
        self.registers
            .get(name)
            .copied()
            .ok_or_else(|| err(format!("value `%{name}` used before definition")))
    }

    /// A scratch register for the instruction being lowered. Scratch
    /// registers sit above every value register and are reused by the next
    /// instruction.
    fn fresh(&mut self) -> Result<u16, LowerError> {
        let register = u16::try_from(self.next_scratch)
            .map_err(|_| err("function needs more than 65535 registers"))?;
        self.next_scratch += 1;
        self.register_count = self.register_count.max(self.next_scratch);
        Ok(register)
    }

    fn pool(&mut self, constant: Const) -> Result<i32, LowerError> {
        let index = if let Some(&index) = self.consts.index.get(&constant) {
            index
        } else {
            let index = self.consts.values.len();
            self.consts.values.push(constant.clone());
            self.consts.index.insert(constant, index);
            index
        };
        i32::try_from(index).map_err(|_| err("constant pool exceeds 2^31 entries"))
    }

    fn width_of(&self, name: &Name) -> Result<WidthCode, LowerError> {
        let ty = self
            .types
            .get(name)
            .ok_or_else(|| err(format!("cannot type `%{name}`")))?;
        let bits = integer_width(ty).ok_or_else(|| err("expected integer operand"))?;
        WidthCode::from_bits(bits).ok_or_else(|| err(format!("unsupported integer width {bits}")))
    }

    fn lower_block(&mut self, block: &Block) -> Result<(), LowerError> {
        let mut index = 0;
        while index < block.instructions.len() {
            let instruction = &block.instructions[index];
            self.next_scratch = self.value_registers;
            if let Some(next) = block.instructions.get(index + 1)
                && instruction.results.len() == 1
                && let Some(result) = instruction.results.first()
                && let Operation::IntegerCompare {
                    predicate,
                    lhs,
                    rhs,
                } = &instruction.operation
                && let Operation::ConditionalBranch(branch) = &next.operation
                && result == &branch.condition
            {
                self.lower_compare_branch(
                    result,
                    *predicate,
                    lhs,
                    rhs,
                    &branch.then_target,
                    &branch.else_target,
                )?;
                index += 2;
                continue;
            }
            self.lower_operation(&instruction.results, &instruction.operation)?;
            index += 1;
        }
        Ok(())
    }

    fn lower_operation(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Result<(), LowerError> {
        if let Some(result) = self.lower_value(results, operation) {
            return result;
        }
        if let Some(result) = self.lower_access(operation) {
            return result;
        }
        if let Some(result) = self.lower_ownership(operation) {
            return result;
        }
        if let Some(result) = self.lower_memory(results, operation) {
            return result;
        }
        if let Some(result) = self.lower_unmanaged(results, operation) {
            return result;
        }
        if let Some(result) = self.lower_atomic(results, operation) {
            return result;
        }
        if let Some(result) = self.lower_call_operation(results, operation) {
            return result;
        }
        if let Some(result) = self.lower_control(operation) {
            return result;
        }
        Err(err(format!("operation not yet lowered: {operation:?}")))
    }

    fn lower_value(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::ConstInt { ty, value } => self.lower_const_int(results, ty, *value),
            Operation::ConstBool(value) => {
                let dest = match self.result(results) {
                    Ok(dest) => dest,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::from_bx(Opcode::LoadI, dest, i32::from(*value)));
                Ok(())
            }
            Operation::ConstNull => {
                let dest = match self.result(results) {
                    Ok(dest) => dest,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::abc(Opcode::LoadNull, dest, 0, 0));
                Ok(())
            }
            Operation::ConstFloat { ty, literal } => self.lower_const_float(results, ty, literal),
            Operation::ConstZero(ty) => self.lower_const_zero(results, ty),
            Operation::ConstString(value) => self.lower_const_string(results, value),
            Operation::Binary {
                operator,
                overflow,
                signed,
                lhs,
                rhs,
            } => self.lower_binary(results, *operator, *overflow, *signed, lhs, rhs),
            Operation::BinaryOverflow {
                operator,
                signed,
                lhs,
                rhs,
            } => self.lower_binary_overflow(results, *operator, *signed, lhs, rhs),
            Operation::IntegerCompare {
                predicate,
                lhs,
                rhs,
            } => self.lower_icmp(results, *predicate, lhs, rhs),
            Operation::FloatBinary { operator, lhs, rhs } => {
                self.lower_float_binary(results, *operator, lhs, rhs)
            }
            Operation::FloatCompare {
                predicate,
                lhs,
                rhs,
            } => self.lower_fcmp(results, *predicate, lhs, rhs),
            Operation::FloatUnary { operator, value } => {
                self.lower_float_unary(results, *operator, value)
            }
            Operation::Move(value)
            | Operation::IntoRaw(value)
            | Operation::FromRaw { pointer: value, .. }
            | Operation::RefAddr(value)
            | Operation::UniqAddr(value)
            | Operation::AddrCast { address: value, .. } => {
                let dest = match self.result(results) {
                    Ok(dest) => dest,
                    Err(error) => return Some(Err(error)),
                };
                let source = match self.register(value) {
                    Ok(source) => source,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::abc(Opcode::Mov, dest, source, 0));
                Ok(())
            }
            Operation::Convert { kind, value, ty } => self.lower_convert(results, *kind, value, ty),
            _ => return None,
        };
        Some(result)
    }

    fn result(&self, results: &[Name]) -> Result<u16, LowerError> {
        match results {
            [single] => self.register(single),
            _ => Err(err("expected exactly one result")),
        }
    }

    fn load_u64_const(&mut self, value: u64) -> Result<u16, LowerError> {
        let register = self.fresh()?;
        if let Ok(small) = i32::try_from(value) {
            self.emit(Instr::from_bx(Opcode::LoadI, register, small));
        } else {
            let index = self.pool(Const::Int(i128::from(value)))?;
            self.emit(Instr::from_bx(Opcode::LoadK, register, index));
        }
        Ok(register)
    }

    fn emit(&mut self, instruction: Instr) {
        self.code.push(instruction);
    }

    pub(super) fn add_branch_site(
        &mut self,
        condition: Option<BranchCondition>,
        then_target: &BlockTarget,
        else_target: Option<&BlockTarget>,
    ) -> Result<i32, LowerError> {
        let then_edge = self.branch_edge(then_target)?;
        let else_edge = else_target
            .map(|target| self.branch_edge(target))
            .transpose()?;
        let site = self.branches.len();
        let index = i32::try_from(site).map_err(|_| err("too many branch sites"))?;
        self.branches.push(BranchSite {
            condition,
            then_edge,
            else_edge,
        });
        self.branch_fixups.push(BranchFixup {
            site,
            then_block: then_target.block.clone(),
            else_block: else_target.map(|target| target.block.clone()),
        });
        Ok(index)
    }

    fn branch_edge(&self, target: &BlockTarget) -> Result<BranchTarget, LowerError> {
        self.delivering_edge(target, &[])
    }

    /// An edge whose leading block parameters receive the `delivered`
    /// registers, ahead of the arguments the target names.
    fn delivering_edge(
        &self,
        target: &BlockTarget,
        delivered: &[u16],
    ) -> Result<BranchTarget, LowerError> {
        let parameters = self
            .types
            .block_parameters(&target.block)
            .ok_or_else(|| err(format!("unknown block `{}`", target.block)))?;
        if parameters.len() != delivered.len() + target.arguments.len() {
            return Err(err("branch argument count mismatch"));
        }
        let sources = delivered
            .iter()
            .map(|register| Ok(*register))
            .chain(target.arguments.iter().map(|source| self.register(source)))
            .collect::<Result<Vec<_>, LowerError>>()?;
        let moves = sources
            .into_iter()
            .zip(parameters)
            .map(|(source, destination)| {
                Ok(RegisterMove {
                    source,
                    destination: self.register(destination)?,
                })
            })
            .collect::<Result<_, LowerError>>()?;
        Ok(BranchTarget { target: 0, moves })
    }

    fn resolve_fixups(&mut self) -> Result<(), LowerError> {
        for fixup in &self.branch_fixups {
            let then_target = *self
                .block_offset
                .get(&fixup.then_block)
                .ok_or_else(|| err(format!("branch to unknown block `{}`", fixup.then_block)))?;
            self.branches[fixup.site].then_edge.target = then_target;
            if let Some(block) = &fixup.else_block {
                let else_target = *self
                    .block_offset
                    .get(block)
                    .ok_or_else(|| err(format!("branch to unknown block `{block}`")))?;
                let edge = self.branches[fixup.site]
                    .else_edge
                    .as_mut()
                    .ok_or_else(|| err("missing else branch edge"))?;
                edge.target = else_target;
            }
        }
        Ok(())
    }
}

fn scalar_width(ty: &Type) -> Result<WidthCode, LowerError> {
    let bits = match ty {
        Type::Integer(bits) | Type::Float(bits) => *bits,
        Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Address(_)
        | Type::Function(_) => 64,
        other => return Err(err(format!("cannot load or store `{other}`"))),
    };
    WidthCode::from_bits(bits).ok_or_else(|| err(format!("unsupported scalar width {bits}")))
}

fn width_of_type(ty: &Type) -> Result<WidthCode, LowerError> {
    let bits = integer_width(ty).ok_or_else(|| err("expected integer type"))?;
    WidthCode::from_bits(bits).ok_or_else(|| err(format!("unsupported integer width {bits}")))
}

#[must_use]
fn integer_predicate_code(predicate: IntegerPredicate) -> u8 {
    match predicate {
        IntegerPredicate::Eq => 0,
        IntegerPredicate::Ne => 1,
        IntegerPredicate::SignedLt => 2,
        IntegerPredicate::SignedLe => 3,
        IntegerPredicate::SignedGt => 4,
        IntegerPredicate::SignedGe => 5,
        IntegerPredicate::UnsignedLt => 6,
        IntegerPredicate::UnsignedLe => 7,
        IntegerPredicate::UnsignedGt => 8,
        IntegerPredicate::UnsignedGe => 9,
    }
}

fn float_predicate_code(predicate: FloatPredicate) -> u8 {
    match predicate {
        FloatPredicate::Eq => 0,
        FloatPredicate::Ne => 1,
        FloatPredicate::Lt => 2,
        FloatPredicate::Le => 3,
        FloatPredicate::Gt => 4,
        FloatPredicate::Ge => 5,
        FloatPredicate::Ordered => 6,
        FloatPredicate::Unordered => 7,
        FloatPredicate::UnorderedEq => 8,
        FloatPredicate::UnorderedNe => 9,
        FloatPredicate::UnorderedLt => 10,
        FloatPredicate::UnorderedLe => 11,
        FloatPredicate::UnorderedGt => 12,
        FloatPredicate::UnorderedGe => 13,
    }
}
