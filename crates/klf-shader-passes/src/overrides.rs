//! Override baking (section 17.3).

use klf_shader::{
    Arena, ArraySize, Expression, Handle, Literal, Module, Override, ScalarKind, Span, Type,
    TypeInner, WorkgroupExtent,
};
use klf_shader_valid::{ConstScalar, ConstValue, Evaluator, ValidationError};
use std::collections::{BTreeMap, HashMap};

/// One override with the value it was baked to.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    pub name: String,
    pub id: Option<u16>,
    pub ty: Handle<Type>,
    pub value: Literal,
    /// Whether the value came from the default rather than a supplied one.
    pub defaulted: bool,
}

fn parse_value(module: &Module, item: &Override, text: &str) -> Result<ConstValue, String> {
    let scalar = module.types[item.ty]
        .inner
        .scalar()
        .ok_or_else(|| format!("override `{}` is not a scalar", item.name))?;
    let bad = || format!("`{text}` is not a value of override `{}`", item.name);
    let value = match (scalar.kind, scalar.width) {
        (ScalarKind::Bool, _) => ConstScalar::Bool(text.parse().map_err(|_| bad())?),
        (ScalarKind::Sint, 8) => ConstScalar::I64(text.parse().map_err(|_| bad())?),
        (ScalarKind::Sint, _) => ConstScalar::I32(text.parse().map_err(|_| bad())?),
        (ScalarKind::Uint, 8) => ConstScalar::U64(text.parse().map_err(|_| bad())?),
        (ScalarKind::Uint, _) => ConstScalar::U32(text.parse().map_err(|_| bad())?),
        (ScalarKind::Float, 8) => ConstScalar::F64(text.parse().map_err(|_| bad())?),
        (ScalarKind::Float, 2) => ConstScalar::F16(text.parse().map_err(|_| bad())?),
        (ScalarKind::Float, _) => ConstScalar::F32(text.parse().map_err(|_| bad())?),
    };
    Ok(ConstValue::Scalar(value))
}

/// Bakes every override: `supplied` maps a name or numeric id to a value's
/// text. Returns what each override became.
///
/// # Errors
/// Returns `shader.override-missing`, `shader.const-not-finite`, or a
/// value that does not parse.
pub fn bake(
    module: &mut Module,
    supplied: &BTreeMap<String, String>,
) -> Result<Vec<Baked>, Vec<ValidationError>> {
    let (baked, values) = evaluate(module, supplied, true)?;
    let problems = apply(module, &values);
    if problems.is_empty() {
        Ok(baked)
    } else {
        Err(problems)
    }
}

/// What each override with a value is, for reflection, when overrides are
/// kept: one without a default is the host's to supply.
///
/// # Errors
/// Returns `shader.const-not-finite` or a value that does not parse.
pub fn defaults(
    module: &Module,
    supplied: &BTreeMap<String, String>,
) -> Result<Vec<Baked>, Vec<ValidationError>> {
    evaluate(module, supplied, false).map(|(baked, _)| baked)
}

type Values = HashMap<Handle<Override>, ConstValue>;

fn evaluate(
    module: &Module,
    supplied: &BTreeMap<String, String>,
    required: bool,
) -> Result<(Vec<Baked>, Values), Vec<ValidationError>> {
    let mut problems = Vec::new();
    let mut values = HashMap::new();
    let mut baked = Vec::new();
    for (handle, item) in module.overrides.iter() {
        let span = module.overrides.span(handle);
        let text = supplied
            .get(&item.name)
            .or_else(|| item.id.and_then(|id| supplied.get(&id.to_string())));
        let value = match (text, item.init) {
            (Some(text), _) => parse_value(module, item, text),
            (None, Some(init)) => Evaluator::new(module, values.clone()).eval(init),
            (None, None) if !required => continue,
            (None, None) => {
                problems.push(ValidationError::keyed(
                    "SV22",
                    "shader.override-missing",
                    format!(
                        "override `{}` has no default and no supplied value",
                        item.name
                    ),
                    span,
                ));
                continue;
            }
        };
        match value {
            Ok(value) if !value.finite() => problems.push(ValidationError::keyed(
                "SV5",
                "shader.const-not-finite",
                format!("override `{}` is not finite", item.name),
                span,
            )),
            Ok(ConstValue::Scalar(scalar)) => {
                baked.push(Baked {
                    name: item.name.clone(),
                    id: item.id,
                    ty: item.ty,
                    value: scalar.to_literal(),
                    defaulted: text.is_none(),
                });
                values.insert(handle, ConstValue::Scalar(scalar));
            }
            Ok(ConstValue::Composite(_)) => problems.push(ValidationError::new(
                "SV5",
                format!("override `{}` is not a scalar", item.name),
                span,
            )),
            Err(message) => problems.push(ValidationError::new("SV5", message, span)),
        }
    }
    if problems.is_empty() {
        Ok((baked, values))
    } else {
        Err(problems)
    }
}

/// Replaces every override by its value, in expressions, array sizes and
/// workgroup sizes, and removes the overrides.
fn apply(
    module: &mut Module,
    values: &HashMap<Handle<Override>, ConstValue>,
) -> Vec<ValidationError> {
    let mut problems = Vec::new();
    let literal = |item: Handle<Override>| values[&item].scalar().map(ConstScalar::to_literal);
    replace_overrides(&mut module.global_expressions, &literal);
    for function in module.functions.iter_mut().map(|(_, function)| function) {
        replace_overrides(&mut function.expressions, &literal);
    }
    for entry in &mut module.entry_points {
        replace_overrides(&mut entry.function.expressions, &literal);
    }
    let mut evaluator = Evaluator::new(module, values.clone());
    let mut sizes = Vec::new();
    for entry in &module.entry_points {
        let mut extents = Vec::new();
        for extent in entry.workgroup_size.iter().flatten() {
            let value = match extent {
                WorkgroupExtent::Override(expression) => evaluator
                    .eval(*expression)
                    .ok()
                    .and_then(|value| value.index()),
                WorkgroupExtent::Fixed(value) => Some(*value),
            };
            if value.is_none() {
                problems.push(ValidationError::new(
                    "SV5",
                    "a workgroup extent does not evaluate",
                    entry.span,
                ));
            }
            extents.push(value.map_or(*extent, WorkgroupExtent::Fixed));
        }
        sizes.push(extents);
    }
    for (entry, extents) in module.entry_points.iter_mut().zip(sizes) {
        if let Some(size) = &mut entry.workgroup_size {
            size.copy_from_slice(&extents);
        }
    }
    let items: Vec<(Type, Span)> = module
        .types
        .iter()
        .map(|(handle, ty)| {
            let mut ty = ty.clone();
            if let TypeInner::Array { size, .. } = &mut ty.inner
                && let ArraySize::Override(item) = *size
                && let Some(count) = values.get(&item).and_then(ConstValue::index)
            {
                *size = ArraySize::Constant(count);
            }
            (ty, module.types.span(handle))
        })
        .collect();
    module.types.rebuild(items);
    module.overrides = Arena::new();
    for shader in &mut module.shaders {
        shader.options.clear();
    }
    problems
}

fn replace_overrides(
    expressions: &mut Arena<Expression>,
    literal: &impl Fn(Handle<Override>) -> Option<Literal>,
) {
    for (_, expression) in expressions.iter_mut() {
        if let Expression::Override(item) = expression
            && let Some(value) = literal(*item)
        {
            *expression = Expression::Literal(value);
        }
    }
}
