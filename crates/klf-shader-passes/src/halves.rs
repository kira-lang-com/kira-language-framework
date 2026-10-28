//! Half floats on a target that has none (section 16.6): every `f16` becomes
//! an `f32`, and every value a module computes as a half is rounded to the
//! nearest half where it is made, through `pack2x16float` and
//! `unpack2x16float`, so it holds the value the half would. What the target
//! cannot keep is the half's storage: a half output is written as a float,
//! and a blend adds floats.

use crate::rewrite::{Inserter, rewrite};
use klf_shader::print::half_to_f32;
use klf_shader::resolve::resolve_function;
use klf_shader::{
    Arena, Capability, Expression, Function, Handle, Literal, MathFunction, Module, Scalar,
    ScalarKind, Span, Type, TypeInner,
};
use std::collections::HashMap;

/// Whether `module` names a half anywhere.
#[must_use]
pub fn uses_halves(module: &Module) -> bool {
    module.capabilities.contains(&Capability::Float16)
        || module
            .types
            .iter()
            .any(|(_, ty)| scalar_of(&ty.inner) == Some(Scalar::F16))
}

/// Rewrites `module` without halves.
pub fn demote_halves(module: &mut Module) {
    let pair = module.types.insert(
        Type {
            name: None,
            inner: TypeInner::Vector {
                size: 2,
                scalar: Scalar::F32,
            },
        },
        Span::default(),
    );
    let mut vectors = [pair; 5];
    for size in [3_u8, 4] {
        vectors[usize::from(size)] = module.types.insert(
            Type {
                name: None,
                inner: TypeInner::Vector {
                    size,
                    scalar: Scalar::F32,
                },
            },
            Span::default(),
        );
    }
    for which in module.function_refs() {
        let Ok(types) = resolve_function(module, module.function(which)) else {
            continue;
        };
        let halves: Vec<Option<u8>> = types
            .iter()
            .map(|ty| match ty.inner(&module.types).scalar_or_vector() {
                Some((size, Scalar::F16)) => Some(size.unwrap_or(1)),
                _ => None,
            })
            .collect();
        demote_function(module.function_mut(which), &halves, &vectors);
    }
    for (_, expression) in module.global_expressions.iter_mut() {
        demote_literal(expression);
    }
    let items: Vec<(Type, Span)> = module
        .types
        .iter()
        .map(|(handle, ty)| {
            let mut ty = ty.clone();
            demote_type(&mut ty.inner);
            (ty, module.types.span(handle))
        })
        .collect();
    module.types.rebuild(items);
    merge_types(module);
    module.capabilities.remove(&Capability::Float16);
}

/// Points every use of a type at the first type equal to it: a demoted
/// `vec4<f16>` is the `vec4<f32>` the module may already have, and types
/// are told apart by handle.
fn merge_types(module: &mut Module) {
    let mut first: HashMap<Type, Handle<Type>> = HashMap::new();
    let map: Vec<Handle<Type>> = module
        .types
        .iter()
        .map(|(handle, ty)| *first.entry(ty.clone()).or_insert(handle))
        .collect();
    let to = |handle: &mut Handle<Type>| *handle = map[handle.index()];
    let items: Vec<(Type, Span)> = module
        .types
        .iter()
        .map(|(handle, ty)| {
            let mut ty = ty.clone();
            match &mut ty.inner {
                TypeInner::Pointer { base, .. } | TypeInner::Array { base, .. } => to(base),
                TypeInner::Struct { members, .. } => {
                    for member in members {
                        to(&mut member.ty);
                    }
                }
                _ => {}
            }
            (ty, module.types.span(handle))
        })
        .collect();
    module.types.rebuild(items);
    for (_, constant) in module.constants.iter_mut() {
        to(&mut constant.ty);
    }
    for (_, item) in module.overrides.iter_mut() {
        to(&mut item.ty);
    }
    for (_, global) in module.globals.iter_mut() {
        to(&mut global.ty);
    }
    merge_expression_types(&mut module.global_expressions, &to);
    for which in module.function_refs() {
        let function = module.function_mut(which);
        for argument in &mut function.arguments {
            to(&mut argument.ty);
        }
        if let Some(result) = &mut function.result {
            to(&mut result.ty);
        }
        for (_, local) in function.locals.iter_mut() {
            to(&mut local.ty);
        }
        merge_expression_types(&mut function.expressions, &to);
    }
}

fn merge_expression_types(expressions: &mut Arena<Expression>, to: &impl Fn(&mut Handle<Type>)) {
    for (_, expression) in expressions.iter_mut() {
        match expression {
            Expression::ZeroValue(ty)
            | Expression::Compose { ty, .. }
            | Expression::Bitcast { ty, .. }
            | Expression::AtomicResult { ty }
            | Expression::WorkgroupUniformLoadResult { ty } => to(ty),
            _ => {}
        }
    }
}

fn scalar_of(inner: &TypeInner) -> Option<Scalar> {
    match *inner {
        TypeInner::Scalar(scalar)
        | TypeInner::Vector { scalar, .. }
        | TypeInner::Matrix { scalar, .. }
        | TypeInner::Atomic(scalar) => Some(scalar),
        _ => None,
    }
}

fn demote_type(inner: &mut TypeInner) {
    match inner {
        TypeInner::Scalar(scalar)
        | TypeInner::Vector { scalar, .. }
        | TypeInner::Matrix { scalar, .. }
        | TypeInner::Atomic(scalar)
            if *scalar == Scalar::F16 =>
        {
            *scalar = Scalar::F32;
        }
        _ => {}
    }
}

fn demote_literal(expression: &mut Expression) {
    if let Expression::Literal(Literal::F16(bits)) = *expression {
        *expression = Expression::Literal(Literal::f32(half_to_f32(bits)));
    }
}

/// Whether `expression` makes a new value rather than passing one along.
fn computes(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::Unary { .. }
            | Expression::Binary { .. }
            | Expression::Math { .. }
            | Expression::Convert { .. }
            | Expression::Derivative { .. }
    )
}

fn demote_function(function: &mut Function, halves: &[Option<u8>], vectors: &[Handle<Type>; 5]) {
    let old = function.expressions.clone();
    rewrite(function, |inserter, handle, mapped| {
        let mut mapped = mapped.clone();
        demote_literal(&mut mapped);
        if let Expression::Convert {
            kind: ScalarKind::Float,
            width: width @ Some(2),
            ..
        } = &mut mapped
        {
            *width = Some(4);
        }
        let Some(size) = halves[handle.index()] else {
            return Some(inserter.push(mapped));
        };
        if !computes(&old[handle]) {
            return Some(inserter.push(mapped));
        }
        let value = inserter.push(mapped);
        Some(rounded(inserter, value, size, vectors))
    });
}

/// `value`, a float when `size` is 1 or a vector of `size` floats, rounded
/// lane by lane to the nearest half.
fn rounded(
    inserter: &mut Inserter<'_>,
    value: Handle<Expression>,
    size: u8,
    vectors: &[Handle<Type>; 5],
) -> Handle<Expression> {
    if size == 1 {
        return rounded_lane(inserter, value, vectors[2]);
    }
    let lanes = (0..u32::from(size))
        .map(|index| {
            let lane = inserter.push(Expression::AccessIndex { base: value, index });
            rounded_lane(inserter, lane, vectors[2])
        })
        .collect();
    inserter.push(Expression::Compose {
        ty: vectors[usize::from(size)],
        components: lanes,
    })
}

fn rounded_lane(
    inserter: &mut Inserter<'_>,
    lane: Handle<Expression>,
    pair: Handle<Type>,
) -> Handle<Expression> {
    let zero = inserter.push(Expression::Literal(Literal::f32(0.0)));
    let packed_pair = inserter.push(Expression::Compose {
        ty: pair,
        components: vec![lane, zero],
    });
    let packed = inserter.push(Expression::Math {
        fun: MathFunction::Pack2x16Float,
        args: vec![packed_pair],
    });
    let unpacked = inserter.push(Expression::Math {
        fun: MathFunction::Unpack2x16Float,
        args: vec![packed],
    });
    inserter.push(Expression::AccessIndex {
        base: unpacked,
        index: 0,
    })
}
