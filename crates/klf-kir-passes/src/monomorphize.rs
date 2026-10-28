//! The generics engine (section 13), by monomorphization. Every call to a
//! generic function with concrete type arguments produces a specialized copy of
//! the callee with the generic parameters substituted, and the call is rewritten
//! to the specialized function. Generic templates are dropped from the output.

use klf_core::Name;
use klf_kir::{Block, Function, Instruction, Module, Operation, Parameter, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Monomorphize `module`: replace generic calls with calls to specialized copies
/// and drop the generic templates.
#[must_use]
pub fn monomorphize(module: &Module) -> Module {
    monomorphize_owned(module.clone())
}

/// [`monomorphize`], taking the module rather than copying it.
#[must_use]
pub fn monomorphize_owned(mut module: Module) -> Module {
    let (templates, plain): (Vec<Function>, Vec<Function>) = std::mem::take(&mut module.functions)
        .into_iter()
        .partition(|function| !function.generics.is_empty());
    let templates: HashMap<&str, &Function> = templates
        .iter()
        .map(|function| (function.name.as_str(), function))
        .collect();
    let generics: HashSet<&str> = templates.keys().copied().collect();

    let mut worklist: Vec<(String, Vec<Type>)> = Vec::new();
    let mut output = Vec::with_capacity(plain.len());
    for mut function in plain {
        rewrite_calls(&mut function, &generics, &mut worklist);
        output.push(function);
    }

    let mut done: HashSet<String> = HashSet::default();
    while let Some((callee, args)) = worklist.pop() {
        let mangled = mangle(&callee, &args);
        if !done.insert(mangled.clone()) {
            continue;
        }
        let Some(template) = templates.get(callee.as_str()) else {
            continue;
        };
        let substitution: HashMap<Name, Type> = template
            .generics
            .iter()
            .map(|generic| generic.name.clone())
            .zip(args.iter().cloned())
            .collect();
        let mut specialized = specialize(template, &substitution, &mangled);
        rewrite_calls(&mut specialized, &generics, &mut worklist);
        output.push(specialized);
    }

    module.functions = output;
    crate::closures::lower_closures(&mut module);
    crate::function_abi::synthesize_adapters(&mut module);
    module
}

fn mangle(callee: &str, args: &[Type]) -> String {
    let mut mangled = callee.to_owned();
    mangled.push('$');
    let parts: Vec<String> = args.iter().map(mangle_type).collect();
    mangled.push_str(&parts.join("."));
    mangled
}

fn mangle_type(ty: &Type) -> String {
    ty.to_string()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn specialize(template: &Function, substitution: &HashMap<Name, Type>, mangled: &str) -> Function {
    let parameters = template
        .parameters
        .iter()
        .map(|parameter| Parameter {
            ty: subst(&parameter.ty, substitution),
            ..parameter.clone()
        })
        .collect();
    let blocks = template
        .blocks
        .iter()
        .map(|block| specialize_block(block, substitution))
        .collect();
    Function {
        name: Name::new(mangled),
        linkage: template.linkage,
        abi: template.abi,
        nounwind: template.nounwind,
        foreign: template.foreign,
        c_prototype: template.c_prototype.clone(),
        attributes: template.attributes.clone(),
        generics: Vec::new(),
        parameters,
        result: subst(&template.result, substitution),
        failure: template.failure.as_ref().map(|ty| subst(ty, substitution)),
        engine: template.engine,
        blocks,
        span: template.span,
    }
}

fn specialize_block(block: &Block, substitution: &HashMap<Name, Type>) -> Block {
    let parameters = block
        .parameters
        .iter()
        .map(|parameter| Parameter {
            ty: subst(&parameter.ty, substitution),
            ..parameter.clone()
        })
        .collect();
    let instructions = block
        .instructions
        .iter()
        .map(|instruction| Instruction {
            operation: subst_operation(&instruction.operation, substitution),
            ..instruction.clone()
        })
        .collect();
    Block {
        parameters,
        instructions,
        ..block.clone()
    }
}

fn subst(ty: &Type, map: &HashMap<Name, Type>) -> Type {
    match ty {
        Type::Generic(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Reference(inner) => Type::Reference(Box::new(subst(inner, map))),
        Type::Unique(inner) => Type::Unique(Box::new(subst(inner, map))),
        Type::Weak(inner) => Type::Weak(Box::new(subst(inner, map))),
        Type::Address(inner) => Type::Address(Box::new(subst(inner, map))),
        Type::Vector(count, inner) => Type::Vector(*count, Box::new(subst(inner, map))),
        Type::Array(count, inner) => Type::Array(*count, Box::new(subst(inner, map))),
        Type::Tuple(elements) => Type::Tuple(elements.iter().map(|e| subst(e, map)).collect()),
        Type::Nominal(name, arguments) => Type::Nominal(
            name.clone(),
            arguments.iter().map(|a| subst(a, map)).collect(),
        ),
        other => other.clone(),
    }
}

fn subst_operation(operation: &Operation, map: &HashMap<Name, Type>) -> Operation {
    match operation {
        Operation::ConstInt { ty, value } => Operation::ConstInt {
            ty: subst(ty, map),
            value: *value,
        },
        Operation::ConstFloat { ty, literal } => Operation::ConstFloat {
            ty: subst(ty, map),
            literal: literal.clone(),
        },
        Operation::ConstZero(ty) => Operation::ConstZero(subst(ty, map)),
        Operation::Convert { kind, value, ty } => Operation::Convert {
            kind: *kind,
            value: value.clone(),
            ty: subst(ty, map),
        },
        Operation::StackAlloc(ty) => Operation::StackAlloc(subst(ty, map)),
        Operation::HeapAllocCounted(ty) => Operation::HeapAllocCounted(subst(ty, map)),
        Operation::HeapAllocUnique(ty) => Operation::HeapAllocUnique(subst(ty, map)),
        Operation::HeapAllocArray { element, count } => Operation::HeapAllocArray {
            element: subst(element, map),
            count: count.clone(),
        },
        Operation::DescriptorReference(ty) => Operation::DescriptorReference(subst(ty, map)),
        Operation::WitnessReference { ty, trait_name } => Operation::WitnessReference {
            ty: subst(ty, map),
            trait_name: trait_name.clone(),
        },
        Operation::Call {
            callee,
            generic_args,
            arguments,
        } => Operation::Call {
            callee: callee.clone(),
            generic_args: generic_args.iter().map(|a| subst(a, map)).collect(),
            arguments: arguments.clone(),
        },
        other => other.clone(),
    }
}

fn rewrite_calls(
    function: &mut Function,
    generics: &HashSet<&str>,
    worklist: &mut Vec<(String, Vec<Type>)>,
) {
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let Operation::Call {
                callee,
                generic_args,
                arguments,
            } = &instruction.operation
                && !generic_args.is_empty()
                && generics.contains(callee.as_str())
            {
                let mangled = mangle(callee.as_str(), generic_args);
                worklist.push((callee.to_string(), generic_args.clone()));
                instruction.operation = Operation::Call {
                    callee: Name::new(mangled),
                    generic_args: Vec::new(),
                    arguments: arguments.clone(),
                };
            }
        }
    }
}
