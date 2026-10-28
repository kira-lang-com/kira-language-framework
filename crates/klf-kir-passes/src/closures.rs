//! Closure calls and closure callbacks (sections 21.8 and 26.4).
//!
//! A RIDL closure is a two-word record, `code` then `ctx`. `code` follows the
//! RIDL convention for the closure's signature and takes `ctx` in the context
//! register. `call.closure` is rewritten here into two loads, a conversion of
//! `code` to that RIDL function type, and a `call.indirect`, so the backends
//! only ever see operations they already run.
//!
//! `closure.callback <index>, fn(...)` names a static platform function whose
//! parameter `index` is a user-data pointer to such a record. Each distinct
//! callback type gets one thunk that recovers the closure from the user data
//! and calls it with the remaining arguments.

use klf_core::Name;
use klf_kir::{
    Block, ConvertKind, Function, FunctionAbi, FunctionType, Instruction, Linkage, LoadMode,
    Module, Operation, Parameter, ParameterType, Type,
};
use klf_source::Span;
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn lower_closures(module: &mut Module) {
    let mut thunks: HashMap<(u32, FunctionType), Name> = HashMap::default();
    let mut generated = Vec::new();
    let mut counter = 0usize;
    for function in &mut module.functions {
        let mut fresh = Fresh::new(function);
        for block in &mut function.blocks {
            let mut rewritten = Vec::with_capacity(block.instructions.len());
            for mut instruction in block.instructions.drain(..) {
                match &instruction.operation {
                    Operation::CallClosure(call) => {
                        let klf_kir::ClosureCall {
                            closure,
                            params,
                            result,
                            arguments,
                        } = &**call;
                        rewritten.extend(closure_call(
                            &mut fresh,
                            closure,
                            params,
                            result,
                            arguments,
                            instruction.results.clone(),
                            instruction.span,
                        ));
                    }
                    Operation::ClosureCallback { user_data, ty } => {
                        let key = (*user_data, ty.clone());
                        let name = thunks.entry(key).or_insert_with(|| {
                            let name = Name::new(format!("__klf_closure_thunk_{counter}"));
                            counter += 1;
                            generated.push(make_thunk(
                                name.clone(),
                                *user_data,
                                ty,
                                instruction.span,
                            ));
                            name
                        });
                        instruction.operation = Operation::FunctionReference(name.clone());
                        rewritten.push(instruction);
                    }
                    _ => rewritten.push(instruction),
                }
            }
            block.instructions = rewritten;
        }
    }
    module.functions.extend(generated);
}

/// Names that cannot collide with a function's existing values.
struct Fresh {
    prefix: String,
    next: usize,
}

impl Fresh {
    fn new(function: &Function) -> Self {
        let mut prefix = "closure".to_owned();
        let taken = |prefix: &str| {
            function.blocks.iter().any(|block| {
                block
                    .parameters
                    .iter()
                    .map(|parameter| &parameter.name)
                    .chain(block.instructions.iter().flat_map(|i| i.results.iter()))
                    .any(|name| name.as_str().starts_with(prefix))
            })
        };
        while taken(&prefix) {
            prefix.push('_');
        }
        Self { prefix, next: 0 }
    }

    fn name(&mut self) -> Name {
        let name = Name::new(format!("{}{}", self.prefix, self.next));
        self.next += 1;
        name
    }
}

fn instruction(results: Vec<Name>, operation: Operation, span: Span) -> Instruction {
    Instruction {
        results,
        operation,
        debug_location: None,
        span,
    }
}

fn closure_call(
    fresh: &mut Fresh,
    record: &Name,
    params: &[Type],
    result: &Type,
    arguments: &[Name],
    results: Vec<Name>,
    span: Span,
) -> Vec<Instruction> {
    let words = Type::Address(Box::new(Type::Array(2, Box::new(Type::Pointer))));
    let typed = fresh.name();
    let mut out = vec![instruction(
        vec![typed.clone()],
        Operation::Convert {
            kind: ConvertKind::Bitcast,
            value: record.clone(),
            ty: words,
        },
        span,
    )];
    let mut word = |index: i128, out: &mut Vec<Instruction>| {
        let position = fresh.name();
        let address = fresh.name();
        let value = fresh.name();
        out.push(instruction(
            vec![position.clone()],
            Operation::ConstInt {
                ty: Type::Integer(64),
                value: index,
            },
            span,
        ));
        out.push(instruction(
            vec![address.clone()],
            Operation::ElemAddr {
                base: typed.clone(),
                index: position,
            },
            span,
        ));
        out.push(instruction(
            vec![value.clone()],
            Operation::Load {
                mode: LoadMode::Trivial,
                address,
            },
            span,
        ));
        value
    };
    let code = word(0, &mut out);
    let context = word(1, &mut out);

    let mut parameters = vec![ParameterType {
        context: true,
        convention: None,
        ty: Type::Pointer,
    }];
    parameters.extend(params.iter().map(|ty| ParameterType {
        context: false,
        convention: None,
        ty: ty.clone(),
    }));
    let signature = FunctionType {
        parameters,
        result: Box::new(result.clone()),
        failure: None,
        abi: FunctionAbi::Ridl,
    };
    let target = fresh.name();
    out.push(instruction(
        vec![target.clone()],
        Operation::Convert {
            kind: ConvertKind::Bitcast,
            value: code,
            ty: Type::Function(signature),
        },
        span,
    ));
    let mut call_params = vec![Type::Pointer];
    call_params.extend(params.iter().cloned());
    let mut call_arguments = vec![context];
    call_arguments.extend(arguments.iter().cloned());
    out.push(instruction(
        results,
        Operation::CallIndirect(Box::new(klf_kir::IndirectCall {
            target,
            params: call_params,
            result: result.clone(),
            arguments: call_arguments,
        })),
        span,
    ));
    out
}

fn make_thunk(name: Name, user_data: u32, ty: &FunctionType, span: Span) -> Function {
    let parameters: Vec<Parameter> = ty
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| Parameter {
            name: Name::new(format!("arg_{index}")),
            convention: parameter.convention,
            context: false,
            ty: parameter.ty.clone(),
            span,
        })
        .collect();
    let record = Name::new(format!("arg_{user_data}"));
    let (params, arguments): (Vec<Type>, Vec<Name>) = parameters
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != user_data as usize)
        .map(|(_, parameter)| (parameter.ty.clone(), parameter.name.clone()))
        .unzip();
    let has_result = *ty.result != Type::Unit;
    let result = Name::new("closure_result");
    let mut fresh = Fresh {
        prefix: "closure".to_owned(),
        next: 0,
    };
    let mut instructions = closure_call(
        &mut fresh,
        &record,
        &params,
        &ty.result,
        &arguments,
        if has_result {
            vec![result.clone()]
        } else {
            Vec::new()
        },
        span,
    );
    instructions.push(instruction(
        Vec::new(),
        Operation::Return(has_result.then_some(result)),
        span,
    ));
    Function {
        name,
        linkage: Linkage::Private,
        abi: FunctionAbi::Platform,
        nounwind: false,
        foreign: false,
        c_prototype: None,
        attributes: Vec::new(),
        generics: Vec::new(),
        parameters: parameters.clone(),
        result: ty.result.as_ref().clone(),
        failure: None,
        engine: klf_kir::Engine::Runtime,
        blocks: vec![Block {
            name: Name::new("closure_body"),
            parameters,
            instructions,
            span,
        }],
        span,
    }
}
