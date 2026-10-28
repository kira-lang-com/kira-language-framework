use klf_core::Name;
use klf_kir::{
    Block, ConvertKind, Function, FunctionAbi, FunctionType, Instruction, Linkage, Module,
    Operation, Parameter, ParameterType, Type,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn synthesize_adapters(module: &mut Module) {
    let conversions = adapted_conversions(module);
    if conversions.is_empty() {
        return;
    }
    let declarations: HashMap<&Name, &Function> = module
        .functions
        .iter()
        .map(|function| (&function.name, function))
        .collect();
    let mut used_symbols = module_symbols(module);
    let mut adapters: HashMap<(Name, FunctionType), Name> = HashMap::default();
    let mut thunks = Vec::new();
    let mut rewrites = Vec::new();
    for conversion in conversions {
        let declaration = declarations[&conversion.callee];
        let key = (conversion.callee.clone(), conversion.destination.clone());
        let thunk = if let Some(thunk) = adapters.get(&key) {
            thunk.clone()
        } else {
            let name =
                unique_thunk_name(declaration, conversion.destination.abi, &mut used_symbols);
            thunks.push(make_thunk(
                name.clone(),
                conversion.callee.clone(),
                declaration,
                &conversion.destination,
            ));
            adapters.insert(key, name.clone());
            name
        };
        rewrites.push((conversion.at, thunk));
    }
    for ((function, block, instruction), thunk) in rewrites {
        module.functions[function].blocks[block].instructions[instruction].operation =
            Operation::FunctionReference(thunk);
    }
    module.functions.extend(thunks);
}

/// A direct function reference converted to a function type its callee's
/// ABI does not match, which an adapter thunk has to bridge.
struct Conversion {
    at: (usize, usize, usize),
    callee: Name,
    destination: FunctionType,
}

fn adapted_conversions(module: &Module) -> Vec<Conversion> {
    let declarations: HashMap<&Name, &Function> = module
        .functions
        .iter()
        .map(|function| (&function.name, function))
        .collect();
    let mut conversions = Vec::new();
    for (function_index, function) in module.functions.iter().enumerate() {
        for (block_index, block) in function.blocks.iter().enumerate() {
            let mut direct_references = HashMap::default();
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                for result in &instruction.results {
                    direct_references.remove(result);
                }
                if let Operation::FunctionReference(callee) = &instruction.operation {
                    if let [result] = instruction.results.as_slice() {
                        direct_references.insert(result.clone(), callee.clone());
                    }
                    continue;
                }
                let Some((value, destination)) = direct_function_conversion(instruction) else {
                    continue;
                };
                let Some(callee) = direct_references.get(&value) else {
                    continue;
                };
                if declarations
                    .get(callee)
                    .is_some_and(|declaration| supports_adapter(declaration, &destination))
                {
                    conversions.push(Conversion {
                        at: (function_index, block_index, instruction_index),
                        callee: callee.clone(),
                        destination,
                    });
                }
            }
        }
    }
    conversions
}

fn direct_function_conversion(instruction: &Instruction) -> Option<(Name, FunctionType)> {
    let Operation::Convert {
        kind: ConvertKind::Bitcast,
        value,
        ty: Type::Function(destination),
    } = &instruction.operation
    else {
        return None;
    };
    Some((value.clone(), destination.clone()))
}

fn supports_adapter(declaration: &Function, destination: &FunctionType) -> bool {
    if !declaration.generics.is_empty()
        || declaration.failure.is_some()
        || declaration
            .parameters
            .iter()
            .any(|parameter| !is_forwardable(&parameter.ty))
        || destination.failure.is_some()
    {
        return false;
    }

    let source = function_type(declaration);
    if !same_signature_except_abi_and_context(&source, destination)
        || source.failure.is_some()
        || !is_supported_result(&declaration.result)
    {
        return false;
    }

    declaration
        .parameters
        .iter()
        .zip(&source.parameters)
        .all(|(parameter, signature)| {
            parameter.convention == signature.convention && parameter.ty == signature.ty
        })
}

fn same_signature_except_abi_and_context(
    source: &FunctionType,
    destination: &FunctionType,
) -> bool {
    let context_count = source
        .parameters
        .iter()
        .filter(|parameter| parameter.context)
        .count();
    if source.abi == destination.abi
        || source.parameters.len() != destination.parameters.len()
        || source.result != destination.result
        || source.failure != destination.failure
        || destination
            .parameters
            .iter()
            .any(|parameter| parameter.context)
    {
        return false;
    }

    if context_count > 0
        && (context_count != 1
            || source.abi != FunctionAbi::Ridl
            || destination.abi != FunctionAbi::Platform)
    {
        return false;
    }

    source
        .parameters
        .iter()
        .zip(&destination.parameters)
        .all(|(source, destination)| {
            source.convention == destination.convention
                && source.ty == destination.ty
                && (!source.context || is_pointer_shaped(&source.ty))
        })
}

fn function_type(function: &Function) -> FunctionType {
    let abi = if function.foreign {
        FunctionAbi::Platform
    } else if function.abi == FunctionAbi::Ridl
        || matches!(function.linkage, Linkage::Exported)
        || function.failure.is_some()
        || function
            .parameters
            .iter()
            .any(|parameter| parameter.context)
    {
        FunctionAbi::Ridl
    } else {
        FunctionAbi::Platform
    };
    FunctionType {
        parameters: function
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                convention: parameter.convention,
                context: parameter.context,
                ty: parameter.ty.clone(),
            })
            .collect(),
        result: Box::new(function.result.clone()),
        failure: function.failure.clone().map(Box::new),
        abi,
    }
}

/// A thunk forwards its arguments and result unchanged, so any concrete type
/// the backends pass to the callee directly can pass through it. Aggregates
/// travel by address in KIR, which is pointer-shaped.
fn is_forwardable(ty: &Type) -> bool {
    match ty {
        Type::Generic(_) => false,
        Type::Vector(_, element) | Type::Array(_, element) => is_forwardable(element),
        Type::Tuple(elements) => elements.iter().all(is_forwardable),
        Type::Nominal(_, arguments) => arguments.iter().all(is_forwardable),
        _ => true,
    }
}

fn is_pointer_shaped(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Pointer | Type::Address(_) | Type::Reference(_) | Type::Unique(_) | Type::Weak(_)
    )
}

fn is_supported_result(ty: &Type) -> bool {
    is_forwardable(ty)
}

fn unique_thunk_name(
    declaration: &Function,
    abi: FunctionAbi,
    used_symbols: &mut HashSet<Name>,
) -> Name {
    let callee: String = declaration
        .name
        .as_str()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let abi = match abi {
        FunctionAbi::Platform => "platform",
        FunctionAbi::Ridl => "ridl",
    };
    let base = format!("__klf_abi_thunk_{callee}_{abi}");
    let mut candidate = base.clone();
    let mut suffix = 0;
    while used_symbols.contains(&Name::new(candidate.as_str())) {
        suffix += 1;
        candidate = format!("{base}_{suffix}");
    }
    let name = Name::new(candidate);
    used_symbols.insert(name.clone());
    name
}

fn module_symbols(module: &Module) -> HashSet<Name> {
    let mut symbols = HashSet::default();
    symbols.extend(
        module
            .functions
            .iter()
            .map(|function| function.name.clone()),
    );
    symbols.extend(module.globals.iter().map(|global| global.name.clone()));
    symbols.extend(module.vtables.iter().map(|vtable| vtable.name.clone()));
    symbols.extend(module.imports.iter().map(|import| import.symbol.clone()));
    symbols.extend(module.exports.iter().map(|export| export.symbol.clone()));
    for witness in &module.witnesses {
        symbols.extend(witness.witnesses.iter().cloned());
    }
    symbols
}

fn make_thunk(
    name: Name,
    callee: Name,
    declaration: &Function,
    signature: &FunctionType,
) -> Function {
    let parameters: Vec<Parameter> = signature
        .parameters
        .iter()
        .zip(&declaration.parameters)
        .enumerate()
        .map(|(index, (parameter, source))| Parameter {
            name: Name::new(format!("arg_{index}")),
            convention: parameter.convention,
            context: false,
            ty: parameter.ty.clone(),
            span: source.span,
        })
        .collect();
    let block_parameters = parameters.clone();
    let arguments = parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    let call_result = Name::new("abi_result");
    let has_result = signature.result.as_ref() != &Type::Unit;
    let span = declaration.span;
    let mut instructions = vec![Instruction {
        results: if has_result {
            vec![call_result.clone()]
        } else {
            Vec::new()
        },
        operation: if declaration.foreign {
            Operation::CallForeign {
                symbol: callee,
                arguments,
            }
        } else {
            Operation::Call {
                callee,
                generic_args: Vec::new(),
                arguments,
            }
        },
        debug_location: None,
        span,
    }];
    instructions.push(Instruction {
        results: Vec::new(),
        operation: Operation::Return(has_result.then_some(call_result)),
        debug_location: None,
        span,
    });

    Function {
        name,
        linkage: Linkage::Private,
        abi: signature.abi,
        nounwind: declaration.nounwind,
        foreign: false,
        c_prototype: None,
        attributes: Vec::new(),
        generics: Vec::new(),
        parameters,
        result: signature.result.as_ref().clone(),
        failure: None,
        engine: declaration.engine,
        blocks: vec![Block {
            name: Name::new("abi_body"),
            parameters: block_parameters,
            instructions,
            span,
        }],
        span,
    }
}
