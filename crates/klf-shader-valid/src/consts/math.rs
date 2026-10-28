//! Compile-time evaluation of math functions.

use super::{ConstValue, Scalar};
use klf_shader::MathFunction;

/// Evaluates a math function over constant arguments.
pub(crate) fn math(fun: MathFunction, args: &[ConstValue]) -> Result<ConstValue, String> {
    let float1 = |op: fn(f64) -> f64| args[0].map(&|value| float_op(value, op));
    let float2 = |op: fn(f64, f64) -> f64| {
        args[0].zip(&args[1], &|a, b| {
            Ok(a.with_float(op(a.float().ok_or("a float")?, b.float().ok_or("a float")?)))
        })
    };
    let dot = |left: &ConstValue, right: &ConstValue| -> Result<f64, String> {
        left.components()
            .iter()
            .zip(right.components())
            .map(|(a, b)| Ok(a.float().ok_or("a float")? * b.float().ok_or("a float")?))
            .sum()
    };
    let first = args
        .first()
        .and_then(|value| value.components().first().copied())
        .ok_or("no arguments")?;
    Ok(match fun {
        MathFunction::Abs => args[0].map(&|value| match value.float() {
            Some(float) => Ok(value.with_float(float.abs())),
            None => Ok(value.with_integer(value.integer().ok_or("abs of a bool")?.abs())),
        })?,
        MathFunction::Min | MathFunction::Max => {
            let pick_max = fun == MathFunction::Max;
            args[0].zip(&args[1], &|a, b| {
                let greater = match (a.float(), b.float()) {
                    (Some(x), Some(y)) => x > y,
                    _ => a.integer() > b.integer(),
                };
                Ok(if greater == pick_max { a } else { b })
            })?
        }
        MathFunction::Clamp => {
            let low = math(MathFunction::Max, &[args[0].clone(), args[1].clone()])?;
            math(MathFunction::Min, &[low, args[2].clone()])?
        }
        MathFunction::Saturate => float1(|x| x.clamp(0.0, 1.0))?,
        MathFunction::Sign => float1(|x| {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        })?,
        MathFunction::Sin => float1(f64::sin)?,
        MathFunction::Cos => float1(f64::cos)?,
        MathFunction::Tan => float1(f64::tan)?,
        MathFunction::Asin => float1(f64::asin)?,
        MathFunction::Acos => float1(f64::acos)?,
        MathFunction::Atan => float1(f64::atan)?,
        MathFunction::Atan2 => float2(f64::atan2)?,
        MathFunction::Sinh => float1(f64::sinh)?,
        MathFunction::Cosh => float1(f64::cosh)?,
        MathFunction::Tanh => float1(f64::tanh)?,
        MathFunction::Asinh => float1(f64::asinh)?,
        MathFunction::Acosh => float1(f64::acosh)?,
        MathFunction::Atanh => float1(f64::atanh)?,
        MathFunction::Radians => float1(f64::to_radians)?,
        MathFunction::Degrees => float1(f64::to_degrees)?,
        MathFunction::Ceil => float1(f64::ceil)?,
        MathFunction::Floor => float1(f64::floor)?,
        MathFunction::Round => float1(f64::round_ties_even)?,
        MathFunction::Fract => float1(|x| x - x.floor())?,
        MathFunction::Trunc => float1(f64::trunc)?,
        MathFunction::Exp => float1(f64::exp)?,
        MathFunction::Exp2 => float1(f64::exp2)?,
        MathFunction::Log => float1(f64::ln)?,
        MathFunction::Log2 => float1(f64::log2)?,
        MathFunction::Pow => float2(f64::powf)?,
        MathFunction::Sqrt => float1(f64::sqrt)?,
        MathFunction::InverseSqrt => float1(|x| 1.0 / x.sqrt())?,
        MathFunction::Step => args[0].zip(&args[1], &|edge, x| {
            Ok(x.with_float(if x.float() >= edge.float() { 1.0 } else { 0.0 }))
        })?,
        MathFunction::Mix => {
            let blended = args[0].zip(&args[1], &|a, b| {
                Ok(Scalar::F64(
                    b.float().ok_or("a float")? - a.float().ok_or("a float")?,
                ))
            })?;
            let scaled = blended.zip(&args[2], &|delta, t| {
                Ok(Scalar::F64(
                    delta.float().unwrap_or(0.0) * t.float().ok_or("a float")?,
                ))
            })?;
            args[0].zip(&scaled, &|a, delta| {
                Ok(a.with_float(a.float().unwrap_or(0.0) + delta.float().unwrap_or(0.0)))
            })?
        }
        MathFunction::SmoothStep => {
            let low = args[0].clone();
            let high = args[1].clone();
            args[2]
                .zip(&low, &|x, low| {
                    Ok(Scalar::F64(
                        x.float().unwrap_or(0.0) - low.float().unwrap_or(0.0),
                    ))
                })?
                .zip(
                    &high.zip(&low, &|high, low| {
                        Ok(Scalar::F64(
                            high.float().unwrap_or(0.0) - low.float().unwrap_or(0.0),
                        ))
                    })?,
                    &|num, den| {
                        let t = (num.float().unwrap_or(0.0) / den.float().unwrap_or(1.0))
                            .clamp(0.0, 1.0);
                        Ok(first.with_float(t * t * (3.0 - 2.0 * t)))
                    },
                )?
        }
        MathFunction::Fma => {
            let product = args[0].zip(&args[1], &|a, b| {
                Ok(a.with_float(a.float().unwrap_or(0.0) * b.float().unwrap_or(0.0)))
            })?;
            product.zip(&args[2], &|a, b| {
                Ok(a.with_float(a.float().unwrap_or(0.0) + b.float().unwrap_or(0.0)))
            })?
        }
        MathFunction::Dot => ConstValue::Scalar(first.with_float(dot(&args[0], &args[1])?)),
        MathFunction::Length => {
            ConstValue::Scalar(first.with_float(dot(&args[0], &args[0])?.sqrt()))
        }
        MathFunction::Distance => {
            let delta = args[0].zip(&args[1], &|a, b| {
                Ok(a.with_float(a.float().unwrap_or(0.0) - b.float().unwrap_or(0.0)))
            })?;
            ConstValue::Scalar(first.with_float(dot(&delta, &delta)?.sqrt()))
        }
        MathFunction::Normalize => {
            let length = dot(&args[0], &args[0])?.sqrt();
            args[0].map(&|value| Ok(value.with_float(value.float().unwrap_or(0.0) / length)))?
        }
        MathFunction::Cross => {
            let (a, b) = (args[0].components(), args[1].components());
            let f = |index: usize, values: &[Scalar]| {
                values
                    .get(index)
                    .and_then(|value| value.float())
                    .unwrap_or(0.0)
            };
            ConstValue::Composite(
                [
                    f(1, &a) * f(2, &b) - f(2, &a) * f(1, &b),
                    f(2, &a) * f(0, &b) - f(0, &a) * f(2, &b),
                    f(0, &a) * f(1, &b) - f(1, &a) * f(0, &b),
                ]
                .into_iter()
                .map(|value| ConstValue::Scalar(first.with_float(value)))
                .collect(),
            )
        }
        other => {
            return Err(format!(
                "`{}` is not evaluated at compile time",
                other.name()
            ));
        }
    })
}

fn float_op(value: Scalar, op: fn(f64) -> f64) -> Result<Scalar, String> {
    let float = value.float().ok_or("a float function of a non-float")?;
    Ok(value.with_float(op(float)))
}
