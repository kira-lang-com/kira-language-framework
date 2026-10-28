//! The statement rules: SV7, SV8, SV9, SV10, SV11, SV19, SV23 on image
//! stores, and the uniformity of control flow.

mod memory;

use crate::ValidationError;
use crate::info::{FunctionInfo, StageOperation};
use crate::uniformity::{Root, root};
use klf_shader::resolve::Resolution;
use klf_shader::{
    AddressSpace, Block, Expression, Function, Handle, Module, SampleLevel, ScalarKind, Span,
    Statement, SwitchValue, TypeInner,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Loop,
    Switch,
    Continuing,
}

pub(crate) struct Walker<'a> {
    module: &'a Module,
    function: &'a Function,
    info: &'a mut FunctionInfo,
    span: Span,
    problems: Vec<ValidationError>,
    emitted: Vec<u8>,
    available: Vec<bool>,
    scopes: Vec<Vec<Handle<Expression>>>,
    next_emit: u32,
    contexts: Vec<Context>,
    uniform: bool,
}

const fn defined_by_statement(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::CallResult(_)
            | Expression::AtomicResult { .. }
            | Expression::WorkgroupUniformLoadResult { .. }
    )
}

fn contains(block: &Block, wanted: &impl Fn(&Statement) -> bool) -> bool {
    block.statements.iter().any(|statement| {
        wanted(statement)
            || match statement {
                Statement::Block(inner) => contains(inner, wanted),
                Statement::If { accept, reject, .. } => {
                    contains(accept, wanted) || contains(reject, wanted)
                }
                Statement::Switch { cases, .. } => {
                    cases.iter().any(|case| contains(&case.body, wanted))
                }
                Statement::Loop {
                    body, continuing, ..
                } => contains(body, wanted) || contains(continuing, wanted),
                _ => false,
            }
    })
}

const fn exits(statement: &Statement) -> bool {
    matches!(statement, Statement::Return { .. } | Statement::Kill)
}

const fn jumps(statement: &Statement) -> bool {
    matches!(
        statement,
        Statement::Return { .. } | Statement::Kill | Statement::Break | Statement::Continue
    )
}

/// Whether a `break` in `block` leaves the loop around it.
fn breaks_loop(block: &Block) -> bool {
    block.statements.iter().any(|statement| match statement {
        Statement::Break => true,
        Statement::Block(inner) => breaks_loop(inner),
        Statement::If { accept, reject, .. } => breaks_loop(accept) || breaks_loop(reject),
        _ => false,
    })
}

/// Whether control never reaches the end of `block`.
fn returns(block: &Block) -> bool {
    match block.statements.last() {
        Some(Statement::Return { .. } | Statement::Kill) => true,
        Some(Statement::Block(inner)) => returns(inner),
        Some(Statement::If { accept, reject, .. }) => returns(accept) && returns(reject),
        Some(Statement::Switch { cases, .. }) => {
            cases.iter().any(|case| case.value == SwitchValue::Default)
                && cases
                    .iter()
                    .all(|case| case.fall_through || returns(&case.body))
        }
        Some(Statement::Loop {
            body,
            continuing,
            break_if,
        }) => break_if.is_none() && !breaks_loop(body) && !breaks_loop(continuing),
        _ => false,
    }
}

impl<'a> Walker<'a> {
    pub(crate) fn new(
        module: &'a Module,
        function: &'a Function,
        info: &'a mut FunctionInfo,
        span: Span,
    ) -> Self {
        let available = function
            .expressions
            .iter()
            .map(|(_, expression)| !expression.needs_emit() && !defined_by_statement(expression))
            .collect();
        Self {
            module,
            function,
            info,
            span,
            problems: Vec::new(),
            emitted: vec![0; function.expressions.len()],
            available,
            scopes: Vec::new(),
            next_emit: 0,
            contexts: Vec::new(),
            uniform: true,
        }
    }

    pub(crate) fn finish(self) -> Vec<ValidationError> {
        self.problems
    }

    fn fail(&mut self, rule: &'static str, message: impl Into<String>, span: Span) {
        let span = if span.is_known() { span } else { self.span };
        self.problems.push(ValidationError::new(
            rule,
            format!("`{}`: {}", self.function.name, message.into()),
            span,
        ));
    }

    fn ty(&self, handle: Handle<Expression>) -> &TypeInner {
        self.info.expressions[handle.index()]
            .ty
            .inner(&self.module.types)
    }

    fn varies(&self, handle: Handle<Expression>) -> bool {
        self.info.expressions[handle.index()].non_uniform
    }

    fn use_expression(&mut self, handle: Handle<Expression>, span: Span) -> bool {
        match self.available.get(handle.index()) {
            Some(true) => {
                self.info.expressions[handle.index()].ref_count += 1;
                true
            }
            Some(false) => {
                self.fail(
                    "SV8",
                    format!("expression {} is used out of scope", handle.index()),
                    span,
                );
                false
            }
            None => {
                self.fail(
                    "SV1",
                    format!("expression {} does not exist", handle.index()),
                    span,
                );
                false
            }
        }
    }

    fn define(&mut self, handle: Handle<Expression>) {
        self.available[handle.index()] = true;
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(handle);
        }
    }

    fn global_use(&mut self, pointer: Handle<Expression>, read: bool, write: bool) {
        if let Some(Root::Global(global)) = root(self.function, pointer) {
            let used = &mut self.info.globals[global.index()];
            used.read |= read;
            used.write |= write;
        }
    }

    pub(crate) fn walk_function(&mut self) {
        let body = &self.function.body;
        self.walk_block(body);
        for (handle, expression) in self.function.expressions.iter() {
            if expression.needs_emit() && self.emitted[handle.index()] == 0 {
                self.fail(
                    "SV7",
                    format!("expression {} is never emitted", handle.index()),
                    self.function.expressions.span(handle),
                );
            }
        }
        match &self.function.result {
            Some(_) if !returns(body) => self.fail(
                "SV11",
                "a path reaches the end without returning a value",
                self.span,
            ),
            _ => {}
        }
        for argument in &self.function.arguments {
            if let TypeInner::Pointer { space, .. } = self.module.types[argument.ty].inner
                && matches!(
                    space,
                    AddressSpace::Uniform | AddressSpace::Storage { .. } | AddressSpace::Immediate
                )
            {
                self.fail(
                    "SV19",
                    "a parameter points into uniform, storage or immediate memory",
                    self.span,
                );
            }
        }
    }

    fn walk_block(&mut self, block: &'a Block) {
        self.scopes.push(Vec::new());
        self.walk_statements(block);
        self.close_scope();
    }

    fn close_scope(&mut self) {
        for handle in self.scopes.pop().unwrap_or_default() {
            self.available[handle.index()] = false;
        }
    }

    fn walk_statements(&mut self, block: &'a Block) {
        for (statement, span) in block.iter() {
            self.statement(statement, span);
        }
    }

    fn emit(&mut self, range: klf_shader::Range<Expression>, span: Span) {
        if range.start < self.next_emit {
            self.fail("SV7", "an emit covers expressions out of arena order", span);
        }
        self.next_emit = self.next_emit.max(range.end);
        for handle in range.iter() {
            let Some(expression) = self.function.expressions.get(handle) else {
                self.fail(
                    "SV1",
                    format!("emit of expression {} that does not exist", handle.index()),
                    span,
                );
                return;
            };
            if !expression.needs_emit() {
                self.fail(
                    "SV7",
                    format!("expression {} is not evaluated by an emit", handle.index()),
                    span,
                );
            }
            self.emitted[handle.index()] += 1;
            if self.emitted[handle.index()] > 1 {
                self.fail(
                    "SV7",
                    format!("expression {} is emitted twice", handle.index()),
                    span,
                );
            }
            for operand in expression.operands() {
                if !self.available[operand.index()] {
                    self.fail(
                        "SV8",
                        format!(
                            "expression {} reads {} out of scope",
                            handle.index(),
                            operand.index()
                        ),
                        span,
                    );
                }
            }
            self.emitted_effects(expression, span);
            self.define(handle);
        }
    }

    fn emitted_effects(&mut self, expression: &Expression, span: Span) {
        match expression {
            Expression::Load { pointer } | Expression::ArrayLength(pointer) => {
                self.global_use(*pointer, true, false);
            }
            Expression::ImageSample {
                image,
                sampler,
                level,
                depth_ref,
                ..
            } => {
                self.global_use(*image, true, false);
                self.global_use(*sampler, true, false);
                if matches!(level, SampleLevel::Auto | SampleLevel::Bias(_))
                    && depth_ref.is_none()
                    && !self.uniform
                {
                    self.info
                        .non_uniform_operations
                        .push(("an implicit-level sample", span));
                }
            }
            Expression::ImageLoad { image, .. } | Expression::ImageQuery { image, .. } => {
                self.global_use(*image, true, false);
            }
            Expression::Derivative { .. } if !self.uniform => {
                self.info
                    .non_uniform_operations
                    .push(("a derivative", span));
            }
            _ => {}
        }
    }

    fn require_uniform(&mut self, what: &'static str, span: Span) {
        if !self.uniform {
            self.info.non_uniform_operations.push((what, span));
        }
    }

    fn branch(&mut self, condition_varies: bool, blocks: &[&'a Block]) {
        let saved = self.uniform;
        self.uniform &= !condition_varies;
        for block in blocks {
            self.walk_block(block);
        }
        self.uniform =
            saved && !(condition_varies && blocks.iter().any(|block| contains(block, &exits)));
    }

    fn statement(&mut self, statement: &'a Statement, span: Span) {
        match statement {
            Statement::Emit(range) => self.emit(*range, span),
            Statement::Block(block) => self.walk_block(block),
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                if self.use_expression(*condition, span)
                    && *self.ty(*condition) != TypeInner::Scalar(klf_shader::Scalar::BOOL)
                {
                    self.fail("SV6", "an if condition is a bool", span);
                }
                let varies = self.varies(*condition);
                self.branch(varies, &[accept, reject]);
            }
            Statement::Switch { selector, cases } => self.switch(*selector, cases, span),
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => self.walk_loop(body, continuing, *break_if, span),
            Statement::Break => {
                if self.contexts.contains(&Context::Continuing) || self.contexts.is_empty() {
                    self.fail(
                        "SV10",
                        "`break` outside a loop or switch, or in a continuing block",
                        span,
                    );
                }
            }
            Statement::Continue => {
                let in_loop = self
                    .contexts
                    .iter()
                    .rev()
                    .find(|context| **context != Context::Switch)
                    == Some(&Context::Loop);
                if !in_loop {
                    self.fail("SV10", "`continue` outside a loop body", span);
                }
            }
            Statement::Return { value } => self.walk_return(*value, span),
            Statement::Kill => {
                if self.contexts.contains(&Context::Continuing) {
                    self.fail("SV10", "`kill` in a continuing block", span);
                }
                self.info.may_kill = true;
                self.info
                    .stage_operations
                    .push((StageOperation::Kill, span));
                self.uniform = false;
            }
            Statement::Store { pointer, value } => self.store(*pointer, *value, span),
            Statement::ImageStore {
                image,
                coordinate,
                array_index,
                value,
            } => {
                self.image_store(*image, *coordinate, *array_index, *value, span);
            }
            Statement::Atomic {
                pointer,
                fun,
                value,
                compare,
                result,
            } => {
                self.atomic(*pointer, *fun, *value, *compare, *result, span);
            }
            Statement::Call {
                function,
                arguments,
                result,
            } => self.call(*function, arguments, *result, span),
            Statement::ControlBarrier(_) | Statement::MemoryBarrier(_) => {
                self.info
                    .stage_operations
                    .push((StageOperation::Barrier, span));
                self.require_uniform("a barrier", span);
            }
            Statement::WorkgroupUniformLoad { pointer, result } => {
                if self.use_expression(*pointer, span) {
                    match self.ty(*pointer) {
                        TypeInner::Pointer {
                            space: AddressSpace::Workgroup,
                            ..
                        } => {}
                        _ => {
                            self.fail("SV9", "workgroup_uniform_load reads workgroup memory", span);
                        }
                    }
                    self.global_use(*pointer, true, false);
                }
                self.info
                    .stage_operations
                    .push((StageOperation::Barrier, span));
                self.require_uniform("workgroup_uniform_load", span);
                self.result(
                    *result,
                    |expression| {
                        matches!(expression, Expression::WorkgroupUniformLoadResult { .. })
                    },
                    span,
                );
            }
        }
    }

    fn result(
        &mut self,
        handle: Handle<Expression>,
        fits: impl Fn(&Expression) -> bool,
        span: Span,
    ) {
        match self.function.expressions.get(handle) {
            Some(expression) if fits(expression) => {
                if self.available[handle.index()] {
                    self.fail(
                        "SV7",
                        format!("result expression {} is defined twice", handle.index()),
                        span,
                    );
                }
                self.define(handle);
            }
            _ => self.fail(
                "SV6",
                format!(
                    "expression {} is not this statement's result",
                    handle.index()
                ),
                span,
            ),
        }
    }

    fn switch(
        &mut self,
        selector: Handle<Expression>,
        cases: &'a [klf_shader::SwitchCase],
        span: Span,
    ) {
        let kind = if self.use_expression(selector, span) {
            match self.ty(selector) {
                TypeInner::Scalar(scalar) if scalar.kind.is_integer() && scalar.width == 4 => {
                    Some(scalar.kind)
                }
                _ => {
                    self.fail("SV6", "a switch selector is an i32 or u32", span);
                    None
                }
            }
        } else {
            None
        };
        let mut seen = Vec::new();
        for case in cases {
            let fits = matches!(
                (case.value, kind),
                (SwitchValue::I32(_), Some(ScalarKind::Sint))
                    | (SwitchValue::U32(_), Some(ScalarKind::Uint))
                    | (SwitchValue::Default, _)
                    | (_, None)
            );
            if !fits {
                self.fail("SV6", "a case value has the selector's type", span);
            }
            if seen.contains(&case.value) {
                self.fail("SV6", "two cases of a switch share a value", span);
            }
            seen.push(case.value);
        }
        if !seen.contains(&SwitchValue::Default) {
            self.fail("SV6", "a switch has a default case", span);
        }
        if cases.last().is_some_and(|case| case.fall_through) {
            self.fail("SV10", "the last case of a switch falls through", span);
        }
        let varies = self.varies(selector);
        self.contexts.push(Context::Switch);
        let bodies: Vec<&Block> = cases.iter().map(|case| &case.body).collect();
        self.branch(varies, &bodies);
        self.contexts.pop();
    }

    fn walk_loop(
        &mut self,
        body: &'a Block,
        continuing: &'a Block,
        break_if: Option<Handle<Expression>>,
        span: Span,
    ) {
        let info = &*self.info;
        let divergent = |statement: &Statement| match statement {
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                info.expressions[condition.index()].non_uniform
                    && (contains(accept, &jumps) || contains(reject, &jumps))
            }
            Statement::Switch { selector, cases } => {
                info.expressions[selector.index()].non_uniform
                    && cases.iter().any(|case| contains(&case.body, &jumps))
            }
            _ => false,
        };
        let varies = contains(body, &divergent)
            || contains(continuing, &divergent)
            || break_if.is_some_and(|condition| self.varies(condition));
        let saved = self.uniform;
        self.uniform &= !varies;
        self.contexts.push(Context::Loop);
        self.scopes.push(Vec::new());
        self.walk_statements(body);
        self.contexts.push(Context::Continuing);
        self.scopes.push(Vec::new());
        self.walk_statements(continuing);
        if let Some(condition) = break_if
            && self.use_expression(condition, span)
            && *self.ty(condition) != TypeInner::Scalar(klf_shader::Scalar::BOOL)
        {
            self.fail("SV6", "a break_if condition is a bool", span);
        }
        self.close_scope();
        self.contexts.pop();
        self.close_scope();
        self.contexts.pop();
        self.uniform = saved && !(varies && contains(body, &exits));
    }

    fn walk_return(&mut self, value: Option<Handle<Expression>>, span: Span) {
        if self.contexts.contains(&Context::Continuing) {
            self.fail("SV10", "`return` in a continuing block", span);
        }
        match (value, &self.function.result) {
            (Some(value), Some(result)) => {
                if self.use_expression(value, span)
                    && !self.info.expressions[value.index()]
                        .ty
                        .same(&Resolution::Handle(result.ty), &self.module.types)
                {
                    self.fail("SV11", "a return value has the wrong type", span);
                }
            }
            (None, None) => {}
            (Some(_), None) => {
                self.fail("SV11", "a function without a result returns a value", span);
            }
            (None, Some(_)) => self.fail("SV11", "a function with a result returns no value", span),
        }
    }
}
