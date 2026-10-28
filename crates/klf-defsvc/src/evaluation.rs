mod buffer;
mod deferred;

use crate::DefinitionServiceState;
use klf_core::Name;
use klf_kir::{Instruction, Module, Operation, Type};
use klf_vm::{EvaluationBudget, Machine, Outcome};
use std::collections::HashMap;

/// A compile-time function whose result is a record written through its
/// first parameter, rebuilt at the call field by field.
pub(super) struct BufferResult {
    name: Name,
    buffer: Option<Buffer>,
}

/// A record field pointing at `length * element_bytes` bytes, which the
/// rebuilt record gets a copy of.
pub(super) struct Buffer {
    allocator: Name,
    data_field: usize,
    length_field: usize,
    element_bytes: u64,
}

impl DefinitionServiceState {
    pub(super) fn function_comptime(&mut self, arguments: &[u64]) -> u64 {
        let Some(name) = self.ll_callee_name(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.comptime_functions.push(name);
        0
    }

    pub(super) fn function_comptime_buffer(&mut self, arguments: &[u64]) -> u64 {
        self.comptime_with_buffer(arguments, Some(1))
    }

    pub(super) fn function_comptime_array(&mut self, arguments: &[u64]) -> u64 {
        self.comptime_with_buffer(arguments, arguments.get(4).copied())
    }

    pub(super) fn function_comptime_record(&mut self, arguments: &[u64]) -> u64 {
        let Some(name) = self.ll_callee_name(arguments.first().copied()) else {
            return u64::MAX;
        };
        self.comptime_buffers.push(BufferResult {
            name: name.clone(),
            buffer: None,
        });
        self.comptime_functions.push(name);
        0
    }

    fn comptime_with_buffer(&mut self, arguments: &[u64], element_bytes: Option<u64>) -> u64 {
        let (
            Some(name),
            Some(allocator),
            Some(data_field),
            Some(length_field),
            Some(element_bytes),
        ) = (
            self.ll_callee_name(arguments.first().copied()),
            self.ll_callee_name(arguments.get(1).copied()),
            arguments
                .get(2)
                .and_then(|value| usize::try_from(*value).ok()),
            arguments
                .get(3)
                .and_then(|value| usize::try_from(*value).ok()),
            element_bytes,
        )
        else {
            return u64::MAX;
        };
        self.comptime_buffers.push(BufferResult {
            name: name.clone(),
            buffer: Some(Buffer {
                allocator,
                data_field,
                length_field,
                element_bytes,
            }),
        });
        self.comptime_functions.push(name);
        0
    }
}

/// A machine for evaluating `program`, its initializers run.
fn initialized_machine<'p>(
    program: &'p klf_vm::Program,
    budget: &mut EvaluationBudget,
) -> Result<Machine<'p>, String> {
    let mut machine = Machine::for_evaluation(program, budget)
        .map_err(|outcome| format!("compile-time heap: {outcome:?}"))?;
    for initializer in &program.initializers {
        let outcome = machine.evaluate(*initializer, &[], budget);
        if !matches!(outcome, Outcome::Return(_)) {
            return Err(format!("compile-time initializer: {outcome:?}"));
        }
    }
    Ok(machine)
}

/// The constant that replaces a call to `callee`, which returned `value`.
fn folded(evaluator: &Module, callee: &Name, value: u64) -> Result<Operation, String> {
    let ty = &evaluator
        .functions
        .iter()
        .find(|function| function.name == *callee)
        .ok_or_else(|| format!("missing compile-time function `{callee}`"))?
        .result;
    match ty {
        Type::Integer(1) => Ok(Operation::ConstBool(value != 0)),
        Type::Integer(_) => Ok(Operation::ConstInt {
            ty: ty.clone(),
            value: i128::from(value.cast_signed()),
        }),
        _ => Err(format!(
            "unsupported compile-time result type of `{callee}`"
        )),
    }
}

/// The constant word `operation` produces, when it produces one.
fn constant(operation: &Operation, constants: &HashMap<Name, u64>) -> Option<u64> {
    match operation {
        Operation::ConstInt { value, .. } => u64::try_from(*value)
            .ok()
            .or_else(|| i64::try_from(*value).ok().map(i64::cast_unsigned)),
        Operation::ConstBool(value) => Some(u64::from(*value)),
        Operation::Copy(source) | Operation::Move(source) => constants.get(source).copied(),
        _ => None,
    }
}

/// Folds the unit's calls to its compile-time functions, or defers them to
/// the merged module when they reach a function another unit defines.
pub(super) fn fold_calls(
    module: &mut Module,
    marked: &[Name],
    buffers: &[BufferResult],
) -> Result<(), String> {
    fold(module, marked, buffers, true)
}

/// Folds the calls the merged module's units deferred.
///
/// # Errors
/// Returns why a deferred call could not be evaluated.
pub(crate) fn fold_deferred(module: &mut Module) -> Result<(), String> {
    let (marked, buffers) = deferred::take(module)?;
    if marked.is_empty() {
        return Ok(());
    }
    fold(module, &marked, &buffers, false)
}

fn fold(
    module: &mut Module,
    marked: &[Name],
    buffers: &[BufferResult],
    may_defer: bool,
) -> Result<(), String> {
    let called = marked_calls(module, marked);
    if called.is_empty() {
        return Ok(());
    }
    let evaluator = evaluator(module, called);
    if may_defer && deferred::calls_undefined(&evaluator, &(0..evaluator.functions.len()).collect())
    {
        deferred::defer(module, marked, buffers);
        return Ok(());
    }
    let program = klf_vm::lower_module(&evaluator).map_err(|error| error.to_string())?;
    // One budget covers every call folded in the module (section 14.4), as
    // large as the one a Kira macro expansion gets.
    let mut budget = EvaluationBudget {
        steps_remaining: 50_000_000,
        memory_bytes: 256 << 20,
    };
    let mut globals = Vec::new();
    for function in &mut module.functions {
        if marked.contains(&function.name) {
            continue;
        }
        for (block_index, block) in function.blocks.iter_mut().enumerate() {
            let mut constants = HashMap::<Name, u64>::new();
            let mut output = Vec::<Instruction>::new();
            for mut instruction in std::mem::take(&mut block.instructions) {
                if let Operation::Call {
                    callee, arguments, ..
                } = &instruction.operation
                    && marked.contains(callee)
                {
                    let descriptor = buffers.iter().find(|buffer| buffer.name == *callee);
                    let values = arguments
                        .iter()
                        .skip(usize::from(descriptor.is_some()))
                        .map(|argument| constants.get(argument).copied())
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            format!("compile-time call to `{callee}` requires constant arguments")
                        })?;
                    let entry = program
                        .function_index(callee.as_str())
                        .ok_or_else(|| format!("missing compile-time function `{callee}`"))?;
                    let mut machine = initialized_machine(&program, &mut budget)?;
                    if let Some(descriptor) = descriptor {
                        let target = arguments
                            .first()
                            .ok_or("compile-time buffer call has no destination")?;
                        let prefix =
                            format!("ct_{}_{}_{}", function.name, block_index, output.len());
                        let call = buffer::BufferCall {
                            entry,
                            arguments: &values,
                            descriptor,
                            destination: target,
                            original: &instruction,
                            prefix,
                        };
                        let (instructions, global) =
                            buffer::evaluate(&evaluator, &mut machine, &mut budget, &call)?;
                        output.extend(instructions);
                        globals.extend(global);
                        continue;
                    }
                    let result = machine.evaluate(entry, &values, &mut budget);
                    let Outcome::Return(Some(value)) = result else {
                        return Err(format!(
                            "compile-time evaluation of `{callee}` failed: {result:?}"
                        ));
                    };
                    instruction.operation = folded(&evaluator, callee, value)?;
                }
                if let [name] = instruction.results.as_slice()
                    && let Some(value) = constant(&instruction.operation, &constants)
                {
                    constants.insert(name.clone(), value);
                }
                output.push(instruction);
            }
            block.instructions = output;
        }
    }
    module.globals.extend(globals);
    Ok(())
}

// The compile-time functions the rest of the module actually calls.
fn marked_calls<'a>(module: &'a Module, marked: &[Name]) -> Vec<&'a Name> {
    let mut called = Vec::new();
    for function in module
        .functions
        .iter()
        .filter(|function| !marked.contains(&function.name))
    {
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            if let Operation::Call { callee, .. } = &instruction.operation
                && marked.contains(callee)
                && !called.contains(&callee)
            {
                called.push(callee);
            }
        }
    }
    called
}

fn initializers(module: &Module) -> impl Iterator<Item = &Name> {
    module
        .functions
        .iter()
        .filter(|function| {
            function
                .attributes
                .contains(&klf_kir::FunctionAttribute::Initializer)
        })
        .map(|function| &function.name)
}

/// Keeps the functions `roots` and the initializers reach: an evaluation
/// costs what it runs rather than the size of the module it runs in.
pub(super) fn retain_reached(module: &mut Module, roots: &[Name]) {
    let reached = module.reachable_functions(roots.iter().chain(initializers(module)));
    let mut position = 0;
    module.functions.retain(|_| {
        position += 1;
        reached.contains(&(position - 1))
    });
}

// The module the VM evaluates: the functions a called compile-time function
// or an initializer reaches.
fn evaluator(module: &Module, called: Vec<&Name>) -> Module {
    let needed = needed_initializers(module, &called);
    let reached = module.reachable_functions(called.into_iter().chain(needed.iter().copied()));
    let functions = module
        .functions
        .iter()
        .enumerate()
        .filter(|(position, _)| reached.contains(position))
        .map(|(_, function)| {
            let mut function = function.clone();
            if !needed.contains(&&function.name) {
                function
                    .attributes
                    .retain(|attribute| *attribute != klf_kir::FunctionAttribute::Initializer);
            }
            function
        })
        .collect();
    Module {
        name: module.name.clone(),
        language: module.language.clone(),
        target: module.target.clone(),
        stage: module.stage,
        attributes: module.attributes.clone(),
        types: module.types.clone(),
        globals: module.globals.clone(),
        vtables: module.vtables.clone(),
        traits: module.traits.clone(),
        conformances: module.conformances.clone(),
        imports: module.imports.clone(),
        exports: module.exports.clone(),
        witnesses: module.witnesses.clone(),
        functions,
    }
}

/// The initializers a compile-time call depends on: those setting a global
/// the functions it reaches take the address of, and in turn those that
/// work of theirs needs. A program's other initializers are not run: one in
/// a module that binds a native library calls into it, which no evaluation
/// can reach, and a call that never reads what it sets does not need it.
fn needed_initializers<'a>(module: &'a Module, called: &[&'a Name]) -> Vec<&'a Name> {
    let candidates: Vec<&klf_kir::Function> = module
        .functions
        .iter()
        .filter(|function| {
            function
                .attributes
                .contains(&klf_kir::FunctionAttribute::Initializer)
        })
        .collect();
    let mut globals = addressed_globals(module, called.iter().copied());
    let mut needed: Vec<&'a Name> = Vec::new();
    loop {
        let mut grew = false;
        for initializer in &candidates {
            if needed.contains(&&initializer.name) {
                continue;
            }
            let sets = initializer
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| global_named(&instruction.operation))
                .any(|global| globals.contains(global));
            if sets {
                needed.push(&initializer.name);
                globals.extend(addressed_globals(module, [&initializer.name]));
                grew = true;
            }
        }
        if !grew {
            return needed;
        }
    }
}

/// Every global the functions `roots` reach take the address of.
fn addressed_globals<'a>(
    module: &'a Module,
    roots: impl IntoIterator<Item = &'a Name>,
) -> std::collections::HashSet<&'a Name> {
    module
        .reachable_functions(roots)
        .into_iter()
        .flat_map(|position| &module.functions[position].blocks)
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| global_named(&instruction.operation))
        .collect()
}

fn global_named(operation: &Operation) -> Option<&Name> {
    match operation {
        Operation::GlobalAddress { global, .. } => Some(global),
        _ => None,
    }
}
