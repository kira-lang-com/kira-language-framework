//! Compaction (section 17.1 step 8): functions and globals no remaining
//! entry point reaches are removed; what remains keeps its order.

use klf_shader::visit::{statements, statements_mut};
use klf_shader::{Expression, Function, GlobalVariable, Handle, Module, Statement};
use std::collections::BTreeSet;

fn calls(function: &Function) -> Vec<Handle<Function>> {
    let mut found = Vec::new();
    statements(&function.body, &mut |statement| {
        if let Statement::Call { function, .. } = statement {
            found.push(*function);
        }
    });
    found
}

fn globals_of(function: &Function, into: &mut BTreeSet<Handle<GlobalVariable>>) {
    for (_, expression) in function.expressions.iter() {
        if let Expression::GlobalVariable(global) = expression {
            into.insert(*global);
        }
    }
}

/// The functions every entry point reaches, and the globals they name.
#[must_use]
pub fn reached(module: &Module) -> (BTreeSet<Handle<Function>>, BTreeSet<Handle<GlobalVariable>>) {
    let mut functions = BTreeSet::new();
    let mut globals = BTreeSet::new();
    let mut stack = Vec::new();
    for entry in &module.entry_points {
        stack.extend(calls(&entry.function));
        globals_of(&entry.function, &mut globals);
    }
    while let Some(function) = stack.pop() {
        if functions.insert(function) {
            let body = &module.functions[function];
            stack.extend(calls(body));
            globals_of(body, &mut globals);
        }
    }
    (functions, globals)
}

/// The globals entry point `entry` reaches, in arena order.
#[must_use]
pub fn entry_globals(module: &Module, entry: usize) -> Vec<Handle<GlobalVariable>> {
    let mut only = module.clone();
    only.entry_points = vec![module.entry_points[entry].clone()];
    reached(&only).1.into_iter().collect()
}

/// Removes unreached functions and globals.
pub fn compact(module: &mut Module) {
    let (functions, globals) = reached(module);
    let function_map = module
        .functions
        .retain_mapped(|handle| functions.contains(&handle));
    let global_map = module
        .globals
        .retain_mapped(|handle| globals.contains(&handle));
    let remap = |function: &mut Function| {
        for (_, expression) in function.expressions.iter_mut() {
            match expression {
                Expression::GlobalVariable(global) => {
                    *global = global_map[global.index()].unwrap_or(*global);
                }
                Expression::CallResult(callee) => {
                    *callee = function_map[callee.index()].unwrap_or(*callee);
                }
                _ => {}
            }
        }
        statements_mut(&mut function.body, &mut |statement| {
            if let Statement::Call { function, .. } = statement {
                *function = function_map[function.index()].unwrap_or(*function);
            }
        });
    };
    for (_, function) in module.functions.iter_mut() {
        remap(function);
    }
    for entry in &mut module.entry_points {
        remap(&mut entry.function);
    }
    for shader in &mut module.shaders {
        for group in &mut shader.groups {
            group.globals = group
                .globals
                .iter()
                .filter_map(|global| global_map[global.index()])
                .collect();
        }
    }
}
