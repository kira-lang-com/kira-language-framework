//! Per-function rules: SV1, SV6, SV12 and SV23 on expressions; the
//! statement rules live in `walk`.

use crate::info::{ExpressionInfo, FunctionInfo, ModuleInfo, StageOperation};
use crate::walk::Walker;
use crate::{ValidationError, uniformity};
use klf_shader::resolve::{Context, Resolution};
use klf_shader::{
    ArraySize, Expression, Function, Handle, ImageClass, ImageDim, Module, SampleLevel, ScalarKind,
    Span, Stage, TypeInner,
};

fn in_range(
    module: &Module,
    function: &Function,
    handle: Handle<Expression>,
    expression: &Expression,
) -> Result<(), String> {
    for operand in expression.operands() {
        if operand.index() >= handle.index() {
            return Err(format!(
                "expression {} names a later expression {}",
                handle.index(),
                operand.index()
            ));
        }
    }
    let present = match expression {
        Expression::Constant(constant) => module.constants.get(*constant).is_some(),
        Expression::Override(item) => module.overrides.get(*item).is_some(),
        Expression::GlobalVariable(global) => module.globals.get(*global).is_some(),
        Expression::LocalVariable(local) => function.locals.get(*local).is_some(),
        Expression::FunctionArgument(index) => (*index as usize) < function.arguments.len(),
        Expression::CallResult(callee) => module.functions.get(*callee).is_some(),
        Expression::ZeroValue(ty)
        | Expression::Compose { ty, .. }
        | Expression::Bitcast { ty, .. }
        | Expression::AtomicResult { ty }
        | Expression::WorkgroupUniformLoadResult { ty } => module.types.get(*ty).is_some(),
        _ => true,
    };
    if present {
        Ok(())
    } else {
        Err(format!(
            "expression {} names something that does not exist",
            handle.index()
        ))
    }
}

/// The constant value of an index expression, when it has one.
fn constant_index(
    module: &Module,
    info: &ModuleInfo,
    function: &Function,
    index: Handle<Expression>,
) -> Option<u32> {
    match function.expressions.get(index)? {
        Expression::Literal(literal) => {
            crate::ConstValue::Scalar(crate::consts::Scalar::from_literal(*literal)).index()
        }
        Expression::Constant(constant) => {
            let init = module.constants.get(*constant)?.init;
            info.constants.get(init.index())?.as_ref()?.index()
        }
        _ => None,
    }
}

fn element_count(module: &Module, inner: &TypeInner) -> Option<u32> {
    let pointee = match inner {
        TypeInner::Pointer { base, .. } => &module.types[*base].inner,
        other => other,
    };
    match pointee {
        TypeInner::Vector { size, .. } => Some(u32::from(*size)),
        TypeInner::Matrix { columns, .. } => Some(u32::from(*columns)),
        TypeInner::Array {
            size: ArraySize::Constant(count),
            ..
        } => Some(*count),
        _ => None,
    }
}

const fn coordinate_size(dim: ImageDim) -> u8 {
    match dim {
        ImageDim::D1 => 1,
        ImageDim::D2 => 2,
        ImageDim::D3 | ImageDim::Cube => 3,
    }
}

/// SV23: the image half `check_expression` leaves.
pub(crate) fn image_rule(
    module: &Module,
    resolved: &[Resolution],
    image: Handle<Expression>,
    coordinate: Handle<Expression>,
    sampled: bool,
) -> Result<(), String> {
    let types = &module.types;
    let TypeInner::Image { dim, class, .. } = resolved[image.index()].inner(types) else {
        return Err("an image operation on a value that is not an image".to_owned());
    };
    let wanted = coordinate_size(*dim);
    let (size, scalar) = resolved[coordinate.index()]
        .inner(types)
        .scalar_or_vector()
        .ok_or("image coordinates are a scalar or vector")?;
    let kind_fits = if sampled {
        scalar.kind == ScalarKind::Float
    } else {
        scalar.kind.is_integer()
    };
    if size.unwrap_or(1) != wanted || !kind_fits {
        return Err(format!(
            "a {} image takes {wanted} coordinates of the right kind",
            dim.name()
        ));
    }
    match (sampled, class) {
        (true, ImageClass::Storage { .. }) => Err("a storage image is not sampled".to_owned()),
        (false, ImageClass::Storage { access, .. }) if !access.load => {
            Err("an image_load from a storage image without `load` access".to_owned())
        }
        _ => Ok(()),
    }
}

fn expression_rules(
    module: &Module,
    info: &ModuleInfo,
    function: &Function,
    resolved: &[Resolution],
    expression: &Expression,
) -> Result<(), ValidationError> {
    let sv = |rule, message: String| ValidationError::new(rule, message, Span::default());
    match expression {
        Expression::Access { base, index } => {
            if let (Some(count), Some(value)) = (
                element_count(module, resolved[base.index()].inner(&module.types)),
                constant_index(module, info, function, *index),
            ) && value >= count
            {
                return Err(ValidationError::keyed(
                    "SV12",
                    "shader.const-index",
                    format!("index {value} is out of range for {count} elements"),
                    Span::default(),
                ));
            }
        }
        Expression::AccessIndex { base, index } => {
            if let Some(count) = element_count(module, resolved[base.index()].inner(&module.types))
                && *index >= count
            {
                return Err(ValidationError::keyed(
                    "SV12",
                    "shader.const-index",
                    format!("index {index} is out of range for {count} elements"),
                    Span::default(),
                ));
            }
        }
        Expression::ImageSample {
            image,
            coordinate,
            depth_ref,
            ..
        } => {
            image_rule(module, resolved, *image, *coordinate, true)
                .map_err(|message| sv("SV23", message))?;
            if let TypeInner::Image { class, .. } = resolved[image.index()].inner(&module.types)
                && depth_ref.is_some()
                && !matches!(class, ImageClass::Depth { .. })
            {
                return Err(sv(
                    "SV23",
                    "a depth comparison samples a depth image".to_owned(),
                ));
            }
        }
        Expression::ImageLoad {
            image, coordinate, ..
        } => {
            image_rule(module, resolved, *image, *coordinate, false)
                .map_err(|message| sv("SV23", message))?;
        }
        _ => {}
    }
    klf_shader::check_expression(module, resolved, expression).map_err(|message| sv("SV6", message))
}

fn stage_operation(expression: &Expression) -> Option<StageOperation> {
    match expression {
        Expression::Derivative { .. } => Some(StageOperation::Derivative),
        Expression::ImageSample {
            level: SampleLevel::Auto | SampleLevel::Bias(_),
            depth_ref: None,
            ..
        } => Some(StageOperation::ImplicitSample),
        _ => None,
    }
}

/// Checks one function or entry point and returns what it found.
pub(crate) fn check_function(
    module: &Module,
    info: &ModuleInfo,
    function: &Function,
    stage: Option<Stage>,
    span: Span,
    problems: &mut Vec<ValidationError>,
) -> FunctionInfo {
    let before = problems.len();
    for (handle, expression) in function.expressions.iter() {
        if let Err(message) = in_range(module, function, handle, expression) {
            problems.push(ValidationError::new(
                "SV1",
                format!("`{}`: {message}", function.name),
                function.expressions.span(handle),
            ));
        }
    }
    if problems.len() > before {
        return FunctionInfo::default();
    }
    let context = Context {
        module,
        function: Some(function),
        expressions: &function.expressions,
    };
    let mut resolved = Vec::with_capacity(function.expressions.len());
    for (handle, expression) in function.expressions.iter() {
        let expression_span = function.expressions.span(handle);
        match context.resolve(expression, &resolved) {
            Ok(resolution) => resolved.push(resolution),
            Err(message) => {
                problems.push(ValidationError::new(
                    "SV6",
                    format!("`{}`: {message}", function.name),
                    expression_span,
                ));
                return FunctionInfo::default();
            }
        }
        if let Err(mut error) = expression_rules(module, info, function, &resolved, expression) {
            error.span = if expression_span.is_known() {
                expression_span
            } else {
                span
            };
            error.message = format!("`{}`: {}", function.name, error.message);
            problems.push(error);
        }
    }
    let non_uniform = uniformity::non_uniform(module, function, stage.is_some(), &resolved);
    let mut result = FunctionInfo {
        expressions: resolved
            .into_iter()
            .zip(&non_uniform)
            .map(|(ty, non_uniform)| ExpressionInfo {
                ty,
                ref_count: 0,
                non_uniform: *non_uniform,
            })
            .collect(),
        globals: vec![crate::GlobalUse::default(); module.globals.len()],
        ..FunctionInfo::default()
    };
    for (handle, expression) in function.expressions.iter() {
        for operand in expression.operands() {
            result.expressions[operand.index()].ref_count += 1;
        }
        if let Expression::GlobalVariable(global) = expression
            && module.globals[*global].space == klf_shader::AddressSpace::Workgroup
        {
            result
                .stage_operations
                .push((StageOperation::Workgroup, function.expressions.span(handle)));
        }
        if let Some(operation) = stage_operation(expression) {
            result
                .stage_operations
                .push((operation, function.expressions.span(handle)));
        }
    }
    let mut walker = Walker::new(module, function, &mut result, span);
    walker.walk_function();
    problems.extend(walker.finish());
    result
}
