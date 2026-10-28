//! The expressions and statements of every function, and the unit-wide
//! block numbers statements refer to each other by.

use crate::record::Record;
use crate::tables::{Tables, number};
use crate::values::literal_fields;
use klf_shader::resolve::Resolution;
use klf_shader::{
    Block, Expression, Function, FunctionRef, Handle, ImageQuery, Module, SampleLevel, Span,
    Statement, SwitchValue, TypeInner,
};
use klf_shader_valid::{FunctionInfo, ModuleInfo};

/// The type of each expression, by function number, then of each global
/// expression.
pub(crate) struct Types {
    /// Per function, each expression's type and whether it is a shape no
    /// declared type names (a `Resolution::Value`).
    functions: Vec<Vec<(i64, bool)>>,
    globals: Vec<i64>,
}

/// Interns every expression's type, so that an out reads each as a record
/// of the `types` table.
pub(crate) fn intern_types(module: &mut Module, info: &ModuleInfo) -> Types {
    let mut functions = Vec::new();
    for which in module.function_refs() {
        let resolutions: Vec<_> = info
            .function(which)
            .expressions
            .iter()
            .map(|expression| expression.ty.clone())
            .collect();
        functions.push(
            resolutions
                .iter()
                .map(|resolution| {
                    let shaped = matches!(resolution, Resolution::Value(_));
                    (resolution.handle(&mut module.types).index() as i64, shaped)
                })
                .collect(),
        );
    }
    let globals = info
        .global_types
        .clone()
        .iter()
        .map(|resolution| resolution.handle(&mut module.types).index() as i64)
        .collect();
    Types { functions, globals }
}

/// Builds the expression and statement tables, and answers the span of
/// each statement by block and position, for refusals.
pub(crate) fn build(
    module: &Module,
    info: &ModuleInfo,
    types: &Types,
    tables: &mut Tables,
) -> Vec<Vec<Span>> {
    let mut blocks = Vec::new();
    for (at, which) in module.function_refs().into_iter().enumerate() {
        let function = module.function(which);
        let expressions = function
            .expressions
            .iter()
            .map(|(handle, expression)| {
                let mut record = expression_record(expression);
                let (ty, shaped) = types.functions[at][handle.index()];
                record
                    .int("type", ty)
                    .flag("shaped", shaped)
                    .int(
                        "ref_count",
                        info.function(which).expressions[handle.index()].ref_count,
                    )
                    .flag(
                        "bake",
                        bakes(module, function, info.function(which), handle),
                    )
                    .flag("emits", expression.needs_emit())
                    .int("line", function.expressions.span(handle).line)
                    .optional_text(
                        "named",
                        function
                            .named_expressions
                            .get(&handle.raw())
                            .map(String::as_str),
                    );
                record.finish()
            })
            .collect();
        tables.insert(("expressions", number(module, which)), expressions);
        let body = flatten(module, &function.body, &mut blocks);
        if let Some(records) = tables.get_mut(&("functions", -1)) {
            records[at].int("body", body);
        }
    }
    let mut spans = Vec::with_capacity(blocks.len());
    for (at, block) in blocks.into_iter().enumerate() {
        let (records, block_spans) = block.into_iter().unzip();
        tables.insert(("statements", at as i64), records);
        spans.push(block_spans);
    }
    let globals = module
        .global_expressions
        .iter()
        .map(|(handle, expression)| {
            let mut record = expression_record(expression);
            record
                .int("type", types.globals[handle.index()])
                .int("line", module.global_expressions.span(handle).line);
            record.finish()
        })
        .collect();
    tables.insert(("expressions", -1), globals);
    spans
}

/// The shared temporary rule (D-343): whether an out binds the expression
/// to a temporary where its `emit` evaluates it.
fn bakes(
    module: &Module,
    function: &Function,
    info: &FunctionInfo,
    handle: Handle<Expression>,
) -> bool {
    let expression = &function.expressions[handle];
    let inner = info.expressions[handle.index()].ty.inner(&module.types);
    if !expression.needs_emit()
        || matches!(
            inner,
            TypeInner::Pointer { .. } | TypeInner::Image { .. } | TypeInner::Sampler { .. }
        )
    {
        return false;
    }
    if function.named_expressions.contains_key(&handle.raw()) {
        return true;
    }
    let threshold = match expression {
        Expression::Load { .. }
        | Expression::ImageSample { .. }
        | Expression::ImageLoad { .. }
        | Expression::ImageQuery { .. }
        | Expression::Derivative { .. }
        | Expression::ArrayLength(_) => 1,
        _ => 2,
    };
    info.expressions[handle.index()].ref_count >= threshold
}

fn index<T>(handle: Handle<T>) -> i64 {
    handle.index() as i64
}

fn list(handles: &[Handle<Expression>]) -> Vec<i64> {
    handles.iter().copied().map(index).collect()
}

const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

#[allow(clippy::too_many_lines)]
fn expression_record(expression: &Expression) -> Record {
    let e = |record: &mut Record, field, handle: &Handle<Expression>| {
        record.int(field, index(*handle));
    };
    let mut record;
    match expression {
        Expression::Literal(literal) => {
            record = Record::new("literal");
            literal_fields(&mut record, *literal);
        }
        Expression::Constant(constant) => {
            record = Record::new("constant");
            record.int("constant", index(*constant));
        }
        Expression::Override(item) => {
            record = Record::new("override");
            record.int("override", index(*item));
        }
        Expression::ZeroValue(_) => record = Record::new("zero"),
        Expression::Compose { components, .. } => {
            record = Record::new("compose");
            record.ints("items", list(components));
        }
        Expression::Access { base, index: at } => {
            record = Record::new("access");
            e(&mut record, "base", base);
            e(&mut record, "index", at);
        }
        Expression::AccessIndex { base, index: at } => {
            record = Record::new("access_index");
            e(&mut record, "base", base);
            record.int("index", *at);
        }
        Expression::Splat { size, value } => {
            record = Record::new("splat");
            record.int("lanes", *size);
            e(&mut record, "value", value);
        }
        Expression::Swizzle { vector, pattern } => {
            record = Record::new("swizzle");
            e(&mut record, "vector", vector);
            record
                .ints(
                    "component",
                    pattern.iter().map(|lane| i64::from(*lane)).collect(),
                )
                .text(
                    "pattern",
                    pattern
                        .iter()
                        .map(|lane| COMPONENTS[usize::from(*lane)])
                        .collect::<String>(),
                );
        }
        Expression::FunctionArgument(argument) => {
            record = Record::new("arg");
            record.int("argument", *argument);
        }
        Expression::GlobalVariable(global) => {
            record = Record::new("global");
            record.int("global", index(*global));
        }
        Expression::LocalVariable(local) => {
            record = Record::new("local");
            record.int("local", index(*local));
        }
        Expression::Load { pointer } => {
            record = Record::new("load");
            e(&mut record, "pointer", pointer);
        }
        Expression::ImageSample {
            image,
            sampler,
            gather,
            coordinate,
            array_index,
            offset,
            level,
            depth_ref,
        } => {
            record = Record::new("image_sample");
            e(&mut record, "image", image);
            e(&mut record, "sampler", sampler);
            e(&mut record, "coordinate", coordinate);
            record
                .handle("array_index", *array_index)
                .handle("offset", *offset)
                .handle("depth_ref", *depth_ref);
            if let Some(component) = gather {
                record.int("gather", *component);
            }
            let word = match level {
                SampleLevel::Auto => "auto",
                SampleLevel::Zero => "zero",
                SampleLevel::Exact(value) => {
                    e(&mut record, "level_value", value);
                    "exact"
                }
                SampleLevel::Bias(value) => {
                    e(&mut record, "level_value", value);
                    "bias"
                }
                SampleLevel::Gradient { x, y } => {
                    e(&mut record, "gradient_x", x);
                    e(&mut record, "gradient_y", y);
                    "gradient"
                }
            };
            record.text("level", word);
        }
        Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            sample,
            level,
        } => {
            record = Record::new("image_load");
            e(&mut record, "image", image);
            e(&mut record, "coordinate", coordinate);
            record
                .handle("array_index", *array_index)
                .handle("sample", *sample)
                .handle("level_value", *level);
        }
        Expression::ImageQuery { image, query } => {
            record = Record::new("image_query");
            e(&mut record, "image", image);
            let word = match query {
                ImageQuery::Size { level } => {
                    record.handle("level_value", *level);
                    "size"
                }
                ImageQuery::NumLevels => "levels",
                ImageQuery::NumLayers => "layers",
                ImageQuery::NumSamples => "samples",
            };
            record.text("query", word);
        }
        Expression::Unary { op, expr } => {
            record = Record::new("unary");
            record.text("op", op.name());
            e(&mut record, "expr", expr);
        }
        Expression::Binary { op, left, right } => {
            record = Record::new("binary");
            record.text("op", op.name());
            e(&mut record, "left", left);
            e(&mut record, "right", right);
        }
        Expression::Select {
            condition,
            accept,
            reject,
        } => {
            record = Record::new("select");
            e(&mut record, "condition", condition);
            e(&mut record, "accept", accept);
            e(&mut record, "reject", reject);
        }
        Expression::Derivative {
            axis,
            control,
            expr,
        } => {
            record = Record::new("derivative");
            record
                .text("axis", axis.name())
                .text("control", control.name());
            e(&mut record, "expr", expr);
        }
        Expression::Relational { fun, argument } => {
            record = Record::new("relational");
            record.text("fun", fun.name());
            e(&mut record, "argument", argument);
        }
        Expression::Math { fun, args } => {
            record = Record::new("math");
            record.text("fun", fun.name()).ints("items", list(args));
        }
        Expression::Convert { expr, .. } => {
            record = Record::new("convert");
            e(&mut record, "expr", expr);
        }
        Expression::Bitcast { expr, .. } => {
            record = Record::new("bitcast");
            e(&mut record, "expr", expr);
        }
        Expression::CallResult(function) => {
            record = Record::new("call_result");
            record.int("function", index(*function));
        }
        Expression::AtomicResult { .. } => record = Record::new("atomic_result"),
        Expression::WorkgroupUniformLoadResult { .. } => {
            record = Record::new("workgroup_uniform_load_result");
        }
        Expression::ArrayLength(pointer) => {
            record = Record::new("array_length");
            e(&mut record, "pointer", pointer);
        }
    }
    record
}

/// Numbers `block` and every block inside it, in order of appearance, and
/// records their statements in `blocks`. Answers `block`'s number.
fn flatten(module: &Module, block: &Block, blocks: &mut Vec<Vec<(Record, Span)>>) -> i64 {
    let at = blocks.len();
    blocks.push(Vec::new());
    let records = block
        .iter()
        .map(|(statement, span)| (statement_record(module, statement, span, blocks), span))
        .collect();
    blocks[at] = records;
    at as i64
}

#[allow(clippy::too_many_lines)]
fn statement_record(
    module: &Module,
    statement: &Statement,
    span: Span,
    blocks: &mut Vec<Vec<(Record, Span)>>,
) -> Record {
    let mut record;
    match statement {
        Statement::Emit(range) => {
            record = Record::new("emit");
            record
                .int("first", range.start)
                .int("last", i64::from(range.end) - 1);
        }
        Statement::Block(block) => {
            record = Record::new("block");
            record.int("body", flatten(module, block, blocks));
        }
        Statement::If {
            condition,
            accept,
            reject,
        } => {
            record = Record::new("if");
            record
                .int("condition", index(*condition))
                .int("accept", flatten(module, accept, blocks))
                .int("reject", flatten(module, reject, blocks));
        }
        Statement::Switch { selector, cases } => {
            record = Record::new("switch");
            record.int("selector", index(*selector));
            let numbers = cases
                .iter()
                .map(|case| flatten(module, &case.body, blocks))
                .collect();
            let values = cases
                .iter()
                .map(|case| match case.value {
                    SwitchValue::I32(value) => format!("{value}i32"),
                    SwitchValue::U32(value) => format!("{value}u32"),
                    SwitchValue::Default => "default".to_owned(),
                })
                .collect();
            record
                .ints("case_block", numbers)
                .texts("case_value", values)
                .ints(
                    "case_fall",
                    cases
                        .iter()
                        .map(|case| i64::from(case.fall_through))
                        .collect(),
                )
                .ints(
                    "case_ends",
                    cases
                        .iter()
                        .map(|case| i64::from(case.body.terminates()))
                        .collect(),
                );
        }
        Statement::Loop {
            body,
            continuing,
            break_if,
        } => {
            record = Record::new("loop");
            record
                .int("body", flatten(module, body, blocks))
                .int("continuing", flatten(module, continuing, blocks))
                .handle("break_if", *break_if);
        }
        Statement::Break => record = Record::new("break"),
        Statement::Continue => record = Record::new("continue"),
        Statement::Return { value } => {
            record = Record::new("return");
            record.handle("value", *value);
        }
        Statement::Kill => record = Record::new("kill"),
        Statement::Store { pointer, value } => {
            record = Record::new("store");
            record
                .int("pointer", index(*pointer))
                .int("value", index(*value));
        }
        Statement::ImageStore {
            image,
            coordinate,
            array_index,
            value,
        } => {
            record = Record::new("image_store");
            record
                .int("image", index(*image))
                .int("coordinate", index(*coordinate))
                .handle("array_index", *array_index)
                .int("value", index(*value));
        }
        Statement::Atomic {
            pointer,
            fun,
            value,
            compare,
            result,
        } => {
            record = Record::new("atomic");
            record
                .int("pointer", index(*pointer))
                .text("fun", fun.name())
                .int("value", index(*value))
                .handle("compare", *compare)
                .handle("result", *result);
        }
        Statement::Call {
            function,
            arguments,
            result,
        } => {
            record = Record::new("call");
            record
                .int("function", number(module, FunctionRef::Function(*function)))
                .ints("arguments", list(arguments))
                .handle("result", *result);
        }
        Statement::ControlBarrier(flags) => {
            record = Record::new("control_barrier");
            record.text("flags", flags.words().join(" "));
        }
        Statement::MemoryBarrier(flags) => {
            record = Record::new("memory_barrier");
            record.text("flags", flags.words().join(" "));
        }
        Statement::WorkgroupUniformLoad { pointer, result } => {
            record = Record::new("workgroup_uniform_load");
            record
                .int("pointer", index(*pointer))
                .int("result", index(*result));
        }
    }
    record.int("line", span.line).int("file", span.file);
    record
}
