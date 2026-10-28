//! Which expressions may differ between invocations (section 16.5).

use klf_shader::resolve::Resolution;
use klf_shader::{
    AddressSpace, Binding, Block, BuiltIn, Expression, Function, Handle, LocalVariable, Module,
    Statement, TypeInner,
};

const fn varying(built_in: BuiltIn) -> bool {
    matches!(
        built_in,
        BuiltIn::Position
            | BuiltIn::VertexIndex
            | BuiltIn::InstanceIndex
            | BuiltIn::FrontFacing
            | BuiltIn::SampleIndex
            | BuiltIn::SampleMask
            | BuiltIn::LocalInvocationId
            | BuiltIn::LocalInvocationIndex
            | BuiltIn::GlobalInvocationId
    )
}

fn binding_varies(binding: Option<&Binding>) -> bool {
    match binding {
        Some(Binding::BuiltIn(built_in)) => varying(*built_in),
        Some(Binding::Location { .. }) => true,
        None => false,
    }
}

fn argument_varies(module: &Module, function: &Function, index: u32) -> bool {
    let Some(argument) = function.arguments.get(index as usize) else {
        return false;
    };
    if binding_varies(argument.binding.as_ref()) {
        return true;
    }
    match &module.types[argument.ty].inner {
        TypeInner::Struct { members, .. } => members
            .iter()
            .any(|member| binding_varies(member.binding.as_ref())),
        _ => false,
    }
}

/// The global or local a pointer expression is rooted at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Root {
    Global(Handle<klf_shader::GlobalVariable>),
    Local(Handle<LocalVariable>),
}

pub(crate) fn root(function: &Function, mut pointer: Handle<Expression>) -> Option<Root> {
    loop {
        match function.expressions.get(pointer)? {
            Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => {
                pointer = *base;
            }
            Expression::GlobalVariable(global) => return Some(Root::Global(*global)),
            Expression::LocalVariable(local) => return Some(Root::Local(*local)),
            _ => return None,
        }
    }
}

fn stores<'a>(block: &'a Block, out: &mut Vec<&'a Statement>) {
    for statement in &block.statements {
        match statement {
            Statement::Block(inner) => stores(inner, out),
            Statement::If { accept, reject, .. } => {
                stores(accept, out);
                stores(reject, out);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    stores(&case.body, out);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                stores(body, out);
                stores(continuing, out);
            }
            Statement::Store { .. } | Statement::Call { .. } => out.push(statement),
            _ => {}
        }
    }
}

/// Whether each expression of `function` may vary between invocations.
/// `entry` says whether the function is an entry point, whose bound
/// arguments vary.
pub(crate) fn non_uniform(
    module: &Module,
    function: &Function,
    entry: bool,
    resolved: &[Resolution],
) -> Vec<bool> {
    let mut locals = vec![false; function.locals.len()];
    let mut statements = Vec::new();
    stores(&function.body, &mut statements);
    loop {
        let values = pass(module, function, entry, resolved, &locals, &statements);
        let mut changed = false;
        for statement in &statements {
            if let Statement::Store { pointer, value } = statement
                && values[value.index()]
                && let Some(Root::Local(local)) = root(function, *pointer)
                && !locals[local.index()]
            {
                locals[local.index()] = true;
                changed = true;
            }
        }
        if !changed {
            return values;
        }
    }
}

fn pass(
    module: &Module,
    function: &Function,
    entry: bool,
    resolved: &[Resolution],
    locals: &[bool],
    statements: &[&Statement],
) -> Vec<bool> {
    let mut values = vec![false; function.expressions.len()];
    let call_varies = |values: &[bool], handle: Handle<Expression>| {
        statements.iter().any(|statement| match statement {
            Statement::Call {
                arguments,
                result: Some(result),
                ..
            } => *result == handle && arguments.iter().any(|argument| values[argument.index()]),
            _ => false,
        })
    };
    for (handle, expression) in function.expressions.iter() {
        let varies = match expression {
            Expression::FunctionArgument(index) => {
                entry && argument_varies(module, function, *index)
            }
            Expression::AtomicResult { .. } => true,
            Expression::CallResult(_) => call_varies(&values, handle),
            Expression::Load { pointer } => {
                let writable = match resolved
                    .get(pointer.index())
                    .map(|ty| ty.inner(&module.types))
                {
                    Some(TypeInner::Pointer { space, .. }) => {
                        matches!(space, AddressSpace::Workgroup)
                            || matches!(space, AddressSpace::Storage { access } if access.store)
                    }
                    _ => false,
                };
                let local = match root(function, *pointer) {
                    Some(Root::Local(local)) => locals[local.index()],
                    _ => false,
                };
                writable || local || values[pointer.index()]
            }
            other => other
                .operands()
                .iter()
                .any(|operand| values[operand.index()]),
        };
        values[handle.index()] = varies;
    }
    values
}
