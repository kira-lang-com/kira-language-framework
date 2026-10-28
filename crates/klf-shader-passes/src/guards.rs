//! Integer edge cases (section 15.2, section 17.1 step 6): division and
//! remainder by zero and of the minimum by -1, and float-to-integer
//! conversion, made defined on targets that leave them undefined.

use crate::rewrite::{Inserter, rewrite};
use klf_shader::resolve::resolve_function;
use klf_shader::{
    BinaryOp, Expression, Function, Handle, Literal, MathFunction, Module, RelationalFunction,
    Scalar, ScalarKind,
};

/// Guards every function of `module`.
pub fn guard_integers(module: &mut Module) {
    let refs = module.function_refs();
    for which in refs {
        let Ok(types) = resolve_function(module, module.function(which)) else {
            continue;
        };
        let shapes: Vec<Option<(Option<u8>, Scalar)>> = types
            .iter()
            .map(|ty| ty.inner(&module.types).scalar_or_vector())
            .collect();
        guard_function(module.function_mut(which), &shapes);
    }
}

fn literal(scalar: Scalar, value: i64) -> Literal {
    match (scalar.kind, scalar.width) {
        (ScalarKind::Sint, 8) => Literal::I64(value),
        (ScalarKind::Sint, _) => Literal::I32(value as i32),
        (ScalarKind::Uint, 8) => Literal::U64(value as u64),
        (ScalarKind::Uint, _) => Literal::U32(value as u32),
        (ScalarKind::Float, _) => Literal::f32(value as f32),
        (ScalarKind::Bool, _) => Literal::Bool(value != 0),
    }
}

struct Shaped<'a, 'b> {
    inserter: &'a mut Inserter<'b>,
    size: Option<u8>,
}

impl Shaped<'_, '_> {
    fn constant(&mut self, value: Literal) -> Handle<Expression> {
        let scalar = self.inserter.push(Expression::Literal(value));
        self.widen(scalar)
    }

    fn widen(&mut self, scalar: Handle<Expression>) -> Handle<Expression> {
        match self.size {
            Some(size) => self.inserter.push(Expression::Splat {
                size,
                value: scalar,
            }),
            None => scalar,
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: Handle<Expression>,
        right: Handle<Expression>,
    ) -> Handle<Expression> {
        self.inserter.push(Expression::Binary { op, left, right })
    }
}

fn guard_function(function: &mut Function, shapes: &[Option<(Option<u8>, Scalar)>]) {
    let old = function.expressions.clone();
    rewrite(function, |inserter, handle, mapped| {
        match (&old[handle], mapped) {
            (
                Expression::Binary {
                    op: op @ (BinaryOp::Divide | BinaryOp::Modulo),
                    left: old_left,
                    right: old_right,
                },
                Expression::Binary { left, right, .. },
            ) => {
                let (left_size, scalar) = shapes[old_left.index()]?;
                let (right_size, _) = shapes[old_right.index()]?;
                if !scalar.kind.is_integer() {
                    return None;
                }
                let size = left_size.or(right_size);
                let mut builder = Shaped { inserter, size };
                let left = if left_size.is_none() {
                    builder.widen(*left)
                } else {
                    *left
                };
                let right = if right_size.is_none() {
                    builder.widen(*right)
                } else {
                    *right
                };
                let zero = builder.constant(literal(scalar, 0));
                let mut bad = builder.binary(BinaryOp::Equal, right, zero);
                if scalar.kind == ScalarKind::Sint {
                    let minimum = if scalar.width == 8 {
                        i64::MIN
                    } else {
                        i64::from(i32::MIN)
                    };
                    let minimum = builder.constant(literal(scalar, minimum));
                    let negative_one = builder.constant(literal(scalar, -1));
                    let is_minimum = builder.binary(BinaryOp::Equal, left, minimum);
                    let is_negative_one = builder.binary(BinaryOp::Equal, right, negative_one);
                    let overflow = builder.binary(BinaryOp::And, is_minimum, is_negative_one);
                    bad = builder.binary(BinaryOp::InclusiveOr, bad, overflow);
                }
                let one = builder.constant(literal(scalar, 1));
                let divisor = builder.inserter.push(Expression::Select {
                    condition: bad,
                    accept: one,
                    reject: right,
                });
                Some(builder.binary(*op, left, divisor))
            }
            (
                Expression::Convert {
                    expr: old_expr,
                    kind: kind @ (ScalarKind::Sint | ScalarKind::Uint),
                    width,
                },
                Expression::Convert { expr, .. },
            ) => {
                let (size, source) = shapes[old_expr.index()]?;
                if source != Scalar::F32 || width.is_some_and(|width| width != 4) {
                    return None;
                }
                let (low, high) = match kind {
                    ScalarKind::Sint => (-2_147_483_648.0, 2_147_483_520.0),
                    _ => (0.0, 4_294_967_040.0),
                };
                let mut builder = Shaped { inserter, size };
                let low = builder.constant(Literal::f32(low));
                let high = builder.constant(Literal::f32(high));
                let target = Scalar {
                    kind: *kind,
                    width: 4,
                };
                let zero = builder.constant(literal(target, 0));
                let clamped = builder.inserter.push(Expression::Math {
                    fun: MathFunction::Clamp,
                    args: vec![*expr, low, high],
                });
                let converted = builder.inserter.push(Expression::Convert {
                    expr: clamped,
                    kind: *kind,
                    width: *width,
                });
                let nan = builder.inserter.push(Expression::Relational {
                    fun: RelationalFunction::IsNan,
                    argument: *expr,
                });
                Some(builder.inserter.push(Expression::Select {
                    condition: nan,
                    accept: zero,
                    reject: converted,
                }))
            }
            _ => None,
        }
    });
}
