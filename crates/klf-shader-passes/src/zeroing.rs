//! Workgroup zeroing (section 15.7, section 17.1 step 7): invocation 0
//! stores zero into every workgroup global the entry uses, then the
//! workgroup synchronizes.

use crate::rewrite::rewrite;
use klf_shader::{
    AddressSpace, BarrierFlags, Binding, Block, BuiltIn, Expression, FunctionArgument,
    GlobalVariable, Handle, Literal, Module, Scalar, Span, Stage, Statement, Type, TypeInner,
};

fn holds_atomic(module: &Module, ty: Handle<Type>) -> bool {
    match &module.types[ty].inner {
        TypeInner::Atomic(_) => true,
        TypeInner::Array { base, .. } => holds_atomic(module, *base),
        TypeInner::Struct { members, .. } => {
            members.iter().any(|member| holds_atomic(module, member.ty))
        }
        _ => false,
    }
}

/// Where the entry's local invocation index is: an argument, or a member
/// of a struct argument.
fn local_index(module: &Module, arguments: &[FunctionArgument]) -> Option<(u32, Option<u32>)> {
    let wanted = Some(Binding::BuiltIn(BuiltIn::LocalInvocationIndex));
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if argument.binding == wanted {
            return Some((index as u32, None));
        }
        match &module.types[argument.ty].inner {
            TypeInner::Struct { members, .. } => members
                .iter()
                .position(|member| member.binding == wanted)
                .map(|member| (index as u32, Some(member as u32))),
            _ => None,
        }
    })
}

/// Zeroes the workgroup globals of every compute entry. `used` lists the
/// globals each entry reaches.
pub fn zero_workgroup(module: &mut Module, used: &[Vec<Handle<GlobalVariable>>]) {
    let u32_type = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Scalar(Scalar::U32),
        },
        Span::default(),
    );
    for (index, globals) in used.iter().enumerate() {
        let targets: Vec<Handle<GlobalVariable>> = globals
            .iter()
            .copied()
            .filter(|global| {
                let variable = &module.globals[*global];
                variable.space == AddressSpace::Workgroup && !holds_atomic(module, variable.ty)
            })
            .collect();
        if targets.is_empty() || module.entry_points[index].stage != Stage::Compute {
            continue;
        }
        let found = local_index(module, &module.entry_points[index].function.arguments);
        let types: Vec<Handle<Type>> = targets
            .iter()
            .map(|global| module.globals[*global].ty)
            .collect();
        let entry = &mut module.entry_points[index];
        let span = entry.span;
        let (argument, member) = found.unwrap_or_else(|| {
            entry.function.arguments.push(FunctionArgument {
                name: Some("_ksir_local_index".to_owned()),
                ty: u32_type,
                binding: Some(Binding::BuiltIn(BuiltIn::LocalInvocationIndex)),
            });
            ((entry.function.arguments.len() - 1) as u32, None)
        });
        let plan = Plan {
            argument,
            member,
            targets: targets.iter().copied().zip(types).collect(),
            span,
        };
        let mut prologue = None;
        if entry.function.expressions.is_empty() {
            let expressions = &mut entry.function.expressions;
            prologue = Some(plan.build(&mut |expression| expressions.append(expression, span)));
        } else {
            rewrite(&mut entry.function, |inserter, handle, _| {
                if handle.index() == 0 {
                    prologue = Some(plan.build(&mut |expression| inserter.push(expression)));
                }
                None
            });
        }
        let Some(mut prologue) = prologue else {
            continue;
        };
        let body = std::mem::take(&mut entry.function.body);
        for (statement, statement_span) in body.iter() {
            prologue.push(statement.clone(), statement_span);
        }
        entry.function.body = prologue;
    }
}

struct Plan {
    argument: u32,
    member: Option<u32>,
    targets: Vec<(Handle<GlobalVariable>, Handle<Type>)>,
    span: Span,
}

impl Plan {
    fn build(&self, push: &mut impl FnMut(Expression) -> Handle<Expression>) -> Block {
        let argument = push(Expression::FunctionArgument(self.argument));
        let zero = push(Expression::Literal(Literal::U32(0)));
        let pointers: Vec<(Handle<Expression>, Handle<Expression>)> = self
            .targets
            .iter()
            .map(|(global, ty)| {
                (
                    push(Expression::GlobalVariable(*global)),
                    push(Expression::ZeroValue(*ty)),
                )
            })
            .collect();
        let first = match self.member {
            Some(index) => push(Expression::AccessIndex {
                base: argument,
                index,
            }),
            None => argument,
        };
        let condition = push(Expression::Binary {
            op: klf_shader::BinaryOp::Equal,
            left: first,
            right: zero,
        });
        let emitted = if self.member.is_some() {
            first
        } else {
            condition
        };
        let mut stores = Block::new();
        for (pointer, value) in pointers {
            stores.push(Statement::Store { pointer, value }, self.span);
        }
        let mut prologue = Block::new();
        prologue.push(
            Statement::Emit(klf_shader::Range::new(emitted.raw(), condition.raw() + 1)),
            self.span,
        );
        prologue.push(
            Statement::If {
                condition,
                accept: stores,
                reject: Block::new(),
            },
            self.span,
        );
        prologue.push(
            Statement::ControlBarrier(BarrierFlags {
                workgroup: true,
                ..BarrierFlags::default()
            }),
            self.span,
        );
        prologue
    }
}
