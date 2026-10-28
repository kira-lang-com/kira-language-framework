//! Constant and override evaluation (section 14.4), with the integer edge
//! cases of section 15.2.

mod math;

use crate::ValidationError;
use klf_shader::{
    ArraySize, BinaryOp, Expression, Handle, Literal, Module, Override, RelationalFunction,
    ScalarKind, Type, TypeInner, UnaryOp,
};
use math::math;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F16(f32),
    F32(f32),
    F64(f64),
}

impl Scalar {
    #[must_use]
    pub fn from_literal(literal: Literal) -> Self {
        match literal {
            Literal::Bool(value) => Self::Bool(value),
            Literal::I32(value) => Self::I32(value),
            Literal::U32(value) => Self::U32(value),
            Literal::I64(value) => Self::I64(value),
            Literal::U64(value) => Self::U64(value),
            Literal::F16(bits) => Self::F16(klf_shader::print::half_to_f32(bits)),
            Literal::F32(bits) => Self::F32(f32::from_bits(bits)),
            Literal::F64(bits) => Self::F64(f64::from_bits(bits)),
        }
    }

    #[must_use]
    pub fn to_literal(self) -> Literal {
        match self {
            Self::Bool(value) => Literal::Bool(value),
            Self::I32(value) => Literal::I32(value),
            Self::U32(value) => Literal::U32(value),
            Self::I64(value) => Literal::I64(value),
            Self::U64(value) => Literal::U64(value),
            Self::F16(value) => Literal::F16(klf_shader::parse::f32_to_half(value)),
            Self::F32(value) => Literal::f32(value),
            Self::F64(value) => Literal::f64(value),
        }
    }

    fn zero(scalar: klf_shader::Scalar) -> Self {
        match (scalar.kind, scalar.width) {
            (ScalarKind::Bool, _) => Self::Bool(false),
            (ScalarKind::Sint, 8) => Self::I64(0),
            (ScalarKind::Sint, _) => Self::I32(0),
            (ScalarKind::Uint, 8) => Self::U64(0),
            (ScalarKind::Uint, _) => Self::U32(0),
            (ScalarKind::Float, 2) => Self::F16(0.0),
            (ScalarKind::Float, 8) => Self::F64(0.0),
            (ScalarKind::Float, _) => Self::F32(0.0),
        }
    }

    fn float(self) -> Option<f64> {
        match self {
            Self::F16(value) | Self::F32(value) => Some(f64::from(value)),
            Self::F64(value) => Some(value),
            _ => None,
        }
    }

    fn with_float(self, value: f64) -> Self {
        match self {
            Self::F16(_) => Self::F16(value as f32),
            Self::F64(_) => Self::F64(value),
            _ => Self::F32(value as f32),
        }
    }

    fn integer(self) -> Option<i128> {
        match self {
            Self::I32(value) => Some(i128::from(value)),
            Self::U32(value) => Some(i128::from(value)),
            Self::I64(value) => Some(i128::from(value)),
            Self::U64(value) => Some(i128::from(value)),
            _ => None,
        }
    }

    /// The same kind and width as `self`, wrapped from `value`.
    fn with_integer(self, value: i128) -> Self {
        match self {
            Self::I32(_) => Self::I32(value as i32),
            Self::U32(_) => Self::U32(value as u32),
            Self::I64(_) => Self::I64(value as i64),
            _ => Self::U64(value as u64),
        }
    }

    const fn bits(self) -> u32 {
        match self {
            Self::I64(_) | Self::U64(_) | Self::F64(_) => 64,
            Self::F16(_) => 16,
            _ => 32,
        }
    }

    const fn signed(self) -> bool {
        matches!(self, Self::I32(_) | Self::I64(_))
    }

    /// Whether the value is finite (always, for non-floats).
    #[must_use]
    pub fn finite(self) -> bool {
        self.float().is_none_or(f64::is_finite)
    }
}

/// A constant value: a scalar, or the components of a vector, matrix,
/// array or struct in order.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Scalar(Scalar),
    Composite(Vec<ConstValue>),
}

impl ConstValue {
    #[must_use]
    pub fn finite(&self) -> bool {
        match self {
            Self::Scalar(scalar) => scalar.finite(),
            Self::Composite(parts) => parts.iter().all(Self::finite),
        }
    }

    #[must_use]
    pub const fn scalar(&self) -> Option<Scalar> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            Self::Composite(_) => None,
        }
    }

    /// The value as an index, when it is a non-negative integer.
    #[must_use]
    pub fn index(&self) -> Option<u32> {
        self.scalar()?
            .integer()
            .and_then(|value| u32::try_from(value).ok())
    }

    fn map(&self, op: &impl Fn(Scalar) -> Result<Scalar, String>) -> Result<Self, String> {
        Ok(match self {
            Self::Scalar(scalar) => Self::Scalar(op(*scalar)?),
            Self::Composite(parts) => Self::Composite(
                parts
                    .iter()
                    .map(|part| part.map(op))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    fn zip(
        &self,
        other: &Self,
        op: &impl Fn(Scalar, Scalar) -> Result<Scalar, String>,
    ) -> Result<Self, String> {
        Ok(match (self, other) {
            (Self::Scalar(left), Self::Scalar(right)) => Self::Scalar(op(*left, *right)?),
            (Self::Composite(left), Self::Composite(right)) if left.len() == right.len() => {
                Self::Composite(
                    left.iter()
                        .zip(right)
                        .map(|(left, right)| left.zip(right, op))
                        .collect::<Result<_, _>>()?,
                )
            }
            (Self::Composite(left), right @ Self::Scalar(_)) => Self::Composite(
                left.iter()
                    .map(|left| left.zip(right, op))
                    .collect::<Result<_, _>>()?,
            ),
            (left @ Self::Scalar(_), Self::Composite(right)) => Self::Composite(
                right
                    .iter()
                    .map(|right| left.zip(right, op))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err("operands of different shapes".to_owned()),
        })
    }

    fn components(&self) -> Vec<Scalar> {
        match self {
            Self::Scalar(scalar) => vec![*scalar],
            Self::Composite(parts) => parts.iter().flat_map(Self::components).collect(),
        }
    }
}

/// Evaluates global expressions with the values chosen for overrides.
pub struct Evaluator<'a> {
    pub module: &'a Module,
    pub overrides: HashMap<Handle<Override>, ConstValue>,
    values: Vec<Option<Result<ConstValue, String>>>,
}

impl<'a> Evaluator<'a> {
    #[must_use]
    pub fn new(module: &'a Module, overrides: HashMap<Handle<Override>, ConstValue>) -> Self {
        Self {
            module,
            overrides,
            values: vec![None; module.global_expressions.len()],
        }
    }

    /// The value of a global expression.
    ///
    /// # Errors
    /// Returns why it has none: it reads an override without a value, or an
    /// operation fails.
    pub fn eval(&mut self, handle: Handle<Expression>) -> Result<ConstValue, String> {
        if let Some(Some(value)) = self.values.get(handle.index()) {
            return value.clone();
        }
        let expression = self
            .module
            .global_expressions
            .get(handle)
            .ok_or("a global expression that does not exist")?;
        let result = self.compute(expression);
        if let Some(slot) = self.values.get_mut(handle.index()) {
            *slot = Some(result.clone());
        }
        result
    }

    /// The value of a zero of `ty`.
    ///
    /// # Errors
    /// Fails on a type with no zero.
    pub fn zero(&self, ty: Handle<Type>) -> Result<ConstValue, String> {
        let types = &self.module.types;
        Ok(match &types[ty].inner {
            TypeInner::Scalar(scalar) => ConstValue::Scalar(Scalar::zero(*scalar)),
            TypeInner::Vector { size, scalar } => {
                ConstValue::Composite(vec![
                    ConstValue::Scalar(Scalar::zero(*scalar));
                    usize::from(*size)
                ])
            }
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            } => ConstValue::Composite(vec![
                ConstValue::Composite(vec![
                    ConstValue::Scalar(
                        Scalar::zero(*scalar)
                    );
                    usize::from(*rows)
                ]);
                usize::from(*columns)
            ]),
            TypeInner::Array {
                base,
                size: ArraySize::Constant(count),
                ..
            } => ConstValue::Composite(vec![self.zero(*base)?; *count as usize]),
            TypeInner::Struct { members, .. } => ConstValue::Composite(
                members
                    .iter()
                    .map(|member| self.zero(member.ty))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err("a type with no zero value".to_owned()),
        })
    }

    fn compute(&mut self, expression: &Expression) -> Result<ConstValue, String> {
        Ok(match expression {
            Expression::Literal(literal) => ConstValue::Scalar(Scalar::from_literal(*literal)),
            Expression::Constant(constant) => {
                let init = self
                    .module
                    .constants
                    .get(*constant)
                    .ok_or("unknown constant")?
                    .init;
                self.eval(init)?
            }
            Expression::Override(item) => match self.overrides.get(item) {
                Some(value) => value.clone(),
                None => {
                    return Err(format!(
                        "override `{}` has no value",
                        self.module.overrides[*item].name
                    ));
                }
            },
            Expression::ZeroValue(ty) => self.zero(*ty)?,
            Expression::Compose { ty, components } => {
                let parts = components
                    .iter()
                    .map(|part| self.eval(*part))
                    .collect::<Result<Vec<_>, _>>()?;
                match self.module.types[*ty].inner {
                    TypeInner::Vector { .. } => ConstValue::Composite(
                        parts
                            .iter()
                            .flat_map(ConstValue::components)
                            .map(ConstValue::Scalar)
                            .collect(),
                    ),
                    _ => ConstValue::Composite(parts),
                }
            }
            Expression::Splat { size, value } => {
                ConstValue::Composite(vec![self.eval(*value)?; usize::from(*size)])
            }
            Expression::Swizzle { vector, pattern } => {
                let ConstValue::Composite(parts) = self.eval(*vector)? else {
                    return Err("swizzle of a scalar".to_owned());
                };
                let picked: Vec<ConstValue> = pattern
                    .iter()
                    .map(|component| {
                        parts
                            .get(usize::from(*component))
                            .cloned()
                            .ok_or("swizzle out of range")
                    })
                    .collect::<Result<_, _>>()?;
                match picked.as_slice() {
                    [one] => one.clone(),
                    _ => ConstValue::Composite(picked),
                }
            }
            Expression::Access { base, index } => {
                let index = self.eval(*index)?.index().ok_or("a negative index")?;
                element(self.eval(*base)?, index)?
            }
            Expression::AccessIndex { base, index } => element(self.eval(*base)?, *index)?,
            Expression::Unary { op, expr } => {
                let op = *op;
                self.eval(*expr)?.map(&|value| unary(op, value))?
            }
            Expression::Binary { op, left, right } => {
                let (left, right) = (self.eval(*left)?, self.eval(*right)?);
                let op = *op;
                left.zip(&right, &|left, right| binary(op, left, right))?
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                let condition = self.eval(*condition)?;
                let (accept, reject) = (self.eval(*accept)?, self.eval(*reject)?);
                match (&condition, &accept, &reject) {
                    (ConstValue::Scalar(Scalar::Bool(pick)), _, _) => {
                        if *pick {
                            accept
                        } else {
                            reject
                        }
                    }
                    (
                        ConstValue::Composite(picks),
                        ConstValue::Composite(accepts),
                        ConstValue::Composite(rejects),
                    ) => ConstValue::Composite(
                        picks
                            .iter()
                            .zip(accepts.iter().zip(rejects))
                            .map(|(pick, (accept, reject))| match pick {
                                ConstValue::Scalar(Scalar::Bool(true)) => accept.clone(),
                                _ => reject.clone(),
                            })
                            .collect(),
                    ),
                    _ => return Err("a select's condition is not a bool".to_owned()),
                }
            }
            Expression::Relational { fun, argument } => {
                let value = self.eval(*argument)?;
                match fun {
                    RelationalFunction::All => ConstValue::Scalar(Scalar::Bool(
                        value
                            .components()
                            .iter()
                            .all(|part| *part == Scalar::Bool(true)),
                    )),
                    RelationalFunction::Any => ConstValue::Scalar(Scalar::Bool(
                        value.components().contains(&Scalar::Bool(true)),
                    )),
                    RelationalFunction::IsNan => value
                        .map(&|part| Ok(Scalar::Bool(part.float().is_some_and(f64::is_nan))))?,
                    RelationalFunction::IsInf => value.map(&|part| {
                        Ok(Scalar::Bool(part.float().is_some_and(f64::is_infinite)))
                    })?,
                }
            }
            Expression::Math { fun, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.eval(*arg))
                    .collect::<Result<Vec<_>, _>>()?;
                math(*fun, &args)?
            }
            Expression::Convert { expr, kind, width } => {
                let (kind, width) = (*kind, *width);
                self.eval(*expr)?
                    .map(&|value| Ok(convert(value, kind, width)))?
            }
            Expression::Bitcast { expr, ty } => {
                let target = self.module.types[*ty]
                    .inner
                    .scalar()
                    .ok_or("bitcast to a non-numeric type")?;
                self.eval(*expr)?.map(&|value| bitcast(value, target))?
            }
            _ => return Err("not a constant expression".to_owned()),
        })
    }
}

fn element(value: ConstValue, index: u32) -> Result<ConstValue, String> {
    match value {
        ConstValue::Composite(mut parts) if (index as usize) < parts.len() => {
            Ok(parts.swap_remove(index as usize))
        }
        ConstValue::Composite(parts) => Err(format!(
            "index {index} is out of range for {} elements",
            parts.len()
        )),
        ConstValue::Scalar(_) => Err("indexing a scalar".to_owned()),
    }
}

fn unary(op: UnaryOp, value: Scalar) -> Result<Scalar, String> {
    Ok(match (op, value) {
        (UnaryOp::LogicalNot, Scalar::Bool(value)) => Scalar::Bool(!value),
        (UnaryOp::Negate, value) if value.float().is_some() => {
            value.with_float(-value.float().unwrap_or(0.0))
        }
        (UnaryOp::Negate, value) => {
            value.with_integer(value.integer().ok_or("negating a bool")?.wrapping_neg())
        }
        (UnaryOp::BitwiseNot, value) => {
            value.with_integer(!value.integer().ok_or("bit_not of a non-integer")?)
        }
        (UnaryOp::LogicalNot, _) => return Err("`not` of a non-bool".to_owned()),
    })
}

fn binary(op: BinaryOp, left: Scalar, right: Scalar) -> Result<Scalar, String> {
    if let (Some(a), Some(b)) = (left.float(), right.float()) {
        let single = !matches!(left, Scalar::F64(_));
        let round = |value: f64| {
            if single {
                f64::from(value as f32)
            } else {
                value
            }
        };
        return Ok(match op {
            BinaryOp::Add => left.with_float(round(a + b)),
            BinaryOp::Subtract => left.with_float(round(a - b)),
            BinaryOp::Multiply => left.with_float(round(a * b)),
            BinaryOp::Divide => left.with_float(round(a / b)),
            BinaryOp::Modulo => left.with_float(round(a - b * (a / b).trunc())),
            BinaryOp::Equal => Scalar::Bool(a == b),
            BinaryOp::NotEqual => Scalar::Bool(a != b),
            BinaryOp::Less => Scalar::Bool(a < b),
            BinaryOp::LessEqual => Scalar::Bool(a <= b),
            BinaryOp::Greater => Scalar::Bool(a > b),
            BinaryOp::GreaterEqual => Scalar::Bool(a >= b),
            _ => return Err(format!("`{}` on floats", op.name())),
        });
    }
    if let (Scalar::Bool(a), Scalar::Bool(b)) = (left, right) {
        return Ok(Scalar::Bool(match op {
            BinaryOp::Equal => a == b,
            BinaryOp::NotEqual => a != b,
            BinaryOp::And | BinaryOp::LogicalAnd => a && b,
            BinaryOp::InclusiveOr | BinaryOp::LogicalOr => a || b,
            BinaryOp::ExclusiveOr => a != b,
            _ => return Err(format!("`{}` on bools", op.name())),
        }));
    }
    let (a, b) = (
        left.integer().ok_or("mixed operands")?,
        right.integer().ok_or("mixed operands")?,
    );
    let bits = left.bits();
    let wrap = |value: i128| left.with_integer(value);
    let min = if left.signed() {
        -(1i128 << (bits - 1))
    } else {
        0
    };
    Ok(match op {
        BinaryOp::Add => wrap(a + b),
        BinaryOp::Subtract => wrap(a - b),
        BinaryOp::Multiply => wrap(a.wrapping_mul(b)),
        BinaryOp::Divide => match b {
            0 => left,
            -1 if a == min && left.signed() => left,
            _ => wrap(a / b),
        },
        BinaryOp::Modulo => match b {
            0 => wrap(0),
            -1 if left.signed() => wrap(0),
            _ => wrap(a % b),
        },
        BinaryOp::Equal => Scalar::Bool(a == b),
        BinaryOp::NotEqual => Scalar::Bool(a != b),
        BinaryOp::Less => Scalar::Bool(a < b),
        BinaryOp::LessEqual => Scalar::Bool(a <= b),
        BinaryOp::Greater => Scalar::Bool(a > b),
        BinaryOp::GreaterEqual => Scalar::Bool(a >= b),
        BinaryOp::And => wrap(a & b),
        BinaryOp::InclusiveOr => wrap(a | b),
        BinaryOp::ExclusiveOr => wrap(a ^ b),
        BinaryOp::ShiftLeft => wrap(a << (b.rem_euclid(i128::from(bits)))),
        BinaryOp::ShiftRight => {
            let amount = b.rem_euclid(i128::from(bits));
            if left.signed() {
                wrap(a >> amount)
            } else {
                wrap((a & ((1i128 << bits) - 1)) >> amount)
            }
        }
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
            return Err("logical operator on integers".to_owned());
        }
    })
}

fn convert(value: Scalar, kind: ScalarKind, width: Option<u8>) -> Scalar {
    let width = width.unwrap_or(match value {
        Scalar::I64(_) | Scalar::U64(_) | Scalar::F64(_) => 8,
        Scalar::F16(_) => 2,
        _ => 4,
    });
    let target = klf_shader::Scalar {
        kind,
        width: if kind == ScalarKind::Bool { 1 } else { width },
    };
    let zero = Scalar::zero(target);
    match (value, kind) {
        (Scalar::Bool(flag), ScalarKind::Bool) => Scalar::Bool(flag),
        (Scalar::Bool(flag), ScalarKind::Float) => zero.with_float(f64::from(u8::from(flag))),
        (Scalar::Bool(flag), _) => zero.with_integer(i128::from(flag)),
        (value, ScalarKind::Bool) => Scalar::Bool(
            value
                .float()
                .map_or_else(|| value.integer() != Some(0), |float| float != 0.0),
        ),
        (value, ScalarKind::Float) => match value.float() {
            Some(float) => zero.with_float(float),
            None => zero
                .with_integer(0)
                .with_float(value.integer().unwrap_or(0) as f64),
        },
        (value, _) => match value.float() {
            Some(float) if float.is_nan() => zero,
            Some(float) => {
                let bits = u32::from(width) * 8;
                let (low, high) = if kind == ScalarKind::Sint {
                    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
                } else {
                    (0, (1i128 << bits) - 1)
                };
                zero.with_integer((float.trunc().clamp(low as f64, high as f64)) as i128)
            }
            None => zero.with_integer(value.integer().unwrap_or(0)),
        },
    }
}

fn bitcast(value: Scalar, target: klf_shader::Scalar) -> Result<Scalar, String> {
    let bits: u32 = match value {
        Scalar::I32(value) => value as u32,
        Scalar::U32(value) => value,
        Scalar::F32(value) => value.to_bits(),
        _ => return Err("only 32-bit values are bitcast".to_owned()),
    };
    Ok(match target.kind {
        ScalarKind::Sint => Scalar::I32(bits as i32),
        ScalarKind::Uint => Scalar::U32(bits),
        ScalarKind::Float => Scalar::F32(f32::from_bits(bits)),
        ScalarKind::Bool => return Err("bitcast to bool".to_owned()),
    })
}

/// Evaluates every constant (SV5) and override default, and checks override
/// ids (SV22). Returns the value of each global expression that does not
/// read an override.
pub(crate) fn check_module(
    module: &Module,
    problems: &mut Vec<ValidationError>,
) -> Vec<Option<ConstValue>> {
    let mut evaluator = Evaluator::new(module, HashMap::new());
    for (_, constant) in module.constants.iter() {
        let span = module.global_expressions.span(constant.init);
        match evaluator.eval(constant.init) {
            Ok(value) if !value.finite() => problems.push(ValidationError::keyed(
                "SV5",
                "shader.const-not-finite",
                format!("constant `{}` is not finite", constant.name),
                span,
            )),
            Ok(_) => {}
            Err(message) => problems.push(ValidationError::new(
                "SV5",
                format!("constant `{}` does not evaluate: {message}", constant.name),
                span,
            )),
        }
    }
    let mut ids = HashMap::new();
    for (handle, item) in module.overrides.iter() {
        if let Some(id) = item.id
            && let Some(previous) = ids.insert(id, handle)
        {
            problems.push(ValidationError::new(
                "SV22",
                format!(
                    "overrides `{}` and `{}` share id {id}",
                    module.overrides[previous].name, item.name
                ),
                module.overrides.span(handle),
            ));
        }
    }
    (0..module.global_expressions.len())
        .map(|index| evaluator.eval(Handle::new(index as u32)).ok())
        .collect()
}
