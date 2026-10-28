//! The function builder: expressions, statements and structured control flow.

use super::{BuildError, Result, fail};
use crate::Span;
use crate::arena::{Handle, Range};
use crate::ir::{Block, Expression, Statement, SwitchCase, SwitchValue};
use crate::module::{Function, FunctionArgument, FunctionResult, LocalVariable, Module, Stage};
use crate::resolve::{Context, Resolution};
use crate::types::{Scalar, Type, TypeInner};

enum Open {
    Body(Block),
    If {
        condition: Handle<Expression>,
        span: Span,
        accept: Block,
        reject: Option<Block>,
    },
    Loop {
        span: Span,
        body: Block,
        continuing: Option<Block>,
    },
    Switch {
        selector: Handle<Expression>,
        span: Span,
        cases: Vec<SwitchCase>,
    },
    Case {
        value: SwitchValue,
        fall_through: bool,
        body: Block,
    },
}

impl Open {
    fn block(&mut self) -> Option<&mut Block> {
        match self {
            Self::Body(block) => Some(block),
            Self::If { accept, reject, .. } => Some(reject.as_mut().unwrap_or(accept)),
            Self::Loop {
                body, continuing, ..
            } => Some(continuing.as_mut().unwrap_or(body)),
            Self::Case { body, .. } => Some(body),
            Self::Switch { .. } => None,
        }
    }
}

/// Builds one function body (section 6.3).
pub struct FunctionBuilder {
    pub function: Function,
    pub(super) stage: Option<Stage>,
    pub(super) span: Span,
    stack: Vec<Open>,
    emit_start: u32,
    resolved: Vec<Resolution>,
}

impl FunctionBuilder {
    pub(super) fn new(
        name: &str,
        arguments: Vec<FunctionArgument>,
        result: Option<FunctionResult>,
        stage: Option<Stage>,
        span: Span,
    ) -> Self {
        Self {
            function: Function {
                name: name.to_owned(),
                arguments,
                result,
                ..Function::default()
            },
            stage,
            span,
            stack: vec![Open::Body(Block::new())],
            emit_start: 0,
            resolved: Vec::new(),
        }
    }

    #[must_use]
    pub const fn stage(&self) -> Option<Stage> {
        self.stage
    }

    /// The type of an expression of this function.
    ///
    /// # Errors
    /// Fails on a handle from elsewhere.
    pub fn type_of(&self, expression: Handle<Expression>) -> Result<&Resolution> {
        self.resolved
            .get(expression.index())
            .ok_or_else(|| BuildError(format!("expression {expression:?} is not of this function")))
    }

    /// # Errors
    /// Fails on an unknown type or an initializer that is not of it.
    pub fn local(
        &mut self,
        module: &Module,
        name: Option<&str>,
        ty: crate::arena::Handle<Type>,
        init: Option<Handle<Expression>>,
        span: Span,
    ) -> Result<Handle<LocalVariable>> {
        if module.types.get(ty).is_none() {
            return fail("a local of an unknown type");
        }
        if let Some(init) = init
            && !self
                .type_of(init)?
                .same(&Resolution::Handle(ty), &module.types)
        {
            return fail("a local's initializer is not of its type");
        }
        Ok(self.function.locals.append(
            LocalVariable {
                name: name.map(str::to_owned),
                ty,
                init,
            },
            span,
        ))
    }

    /// Appends an expression, checked against the types of its operands.
    ///
    /// # Errors
    /// Fails on an operand that does not exist or has the wrong type.
    pub fn expression(
        &mut self,
        module: &mut Module,
        expression: Expression,
        span: Span,
    ) -> Result<Handle<Expression>> {
        let count = self.function.expressions.len();
        if expression
            .operands()
            .iter()
            .any(|operand| operand.index() >= count)
        {
            return fail("an operand does not exist in this function");
        }
        if matches!(
            expression,
            Expression::CallResult(_)
                | Expression::AtomicResult { .. }
                | Expression::WorkgroupUniformLoadResult { .. }
        ) {
            return fail("results are made by their statements");
        }
        self.intern_element(module, &expression);
        let resolution = Context {
            module,
            function: Some(&self.function),
            expressions: &self.function.expressions,
        }
        .resolve(&expression, &self.resolved)
        .map_err(BuildError)?;
        crate::check::check(module, &self.resolved, &expression, &resolution)
            .map_err(BuildError)?;
        Ok(self.push_expression(expression, resolution, span))
    }

    /// Interns the element type an access through a pointer names.
    fn intern_element(&self, module: &mut Module, expression: &Expression) {
        let (Expression::Access { base, .. } | Expression::AccessIndex { base, .. }) = expression
        else {
            return;
        };
        let Some(Resolution::Value(TypeInner::Pointer { base: pointee, .. })) =
            self.resolved.get(base.index()).cloned()
        else {
            return;
        };
        let element = match &module.types[pointee].inner {
            TypeInner::Vector { scalar, .. } => TypeInner::Scalar(*scalar),
            TypeInner::Matrix { rows, scalar, .. } => TypeInner::Vector {
                size: *rows,
                scalar: *scalar,
            },
            _ => return,
        };
        module.types.insert(
            Type {
                name: None,
                inner: element,
            },
            Span::default(),
        );
    }

    fn push_expression(
        &mut self,
        expression: Expression,
        resolution: Resolution,
        span: Span,
    ) -> Handle<Expression> {
        if !expression.needs_emit() {
            self.flush();
        }
        let handle = self.function.expressions.append(expression, span);
        self.resolved.push(resolution);
        if !self.function.expressions[handle].needs_emit() {
            self.emit_start = handle.raw() + 1;
        }
        handle
    }

    /// Closes the open emit range into the current block.
    fn flush(&mut self) {
        let end = self.function.expressions.len() as u32;
        if end > self.emit_start {
            let range = Range::new(self.emit_start, end);
            if let Some(block) = self.stack.last_mut().and_then(Open::block) {
                block.push(Statement::Emit(range), Span::default());
            }
        }
        self.emit_start = end;
    }

    fn block(&mut self) -> Result<&mut Block> {
        self.stack.last_mut().and_then(Open::block).ok_or_else(|| {
            BuildError("statements go in a case, not directly in a switch".to_owned())
        })
    }

    /// Appends a statement to the innermost open block.
    ///
    /// # Errors
    /// Fails when the innermost construct is a switch with no open case.
    pub fn statement(&mut self, statement: Statement, span: Span) -> Result<()> {
        self.flush();
        self.block()?.push(statement, span);
        Ok(())
    }

    pub fn name_expression(&mut self, expression: Handle<Expression>, name: &str) {
        self.function
            .named_expressions
            .insert(expression.raw(), name.to_owned());
    }

    /// Appends a call and returns its result expression.
    ///
    /// # Errors
    /// Fails on an argument count or type that does not match.
    pub fn call(
        &mut self,
        module: &Module,
        callee: Handle<crate::module::Function>,
        arguments: Vec<Handle<Expression>>,
        span: Span,
    ) -> Result<Option<Handle<Expression>>> {
        let target = module
            .functions
            .get(callee)
            .ok_or_else(|| BuildError("call of a function that is not finished".to_owned()))?;
        if target.arguments.len() != arguments.len() {
            return fail(format!(
                "`{}` takes {} arguments, not {}",
                target.name,
                target.arguments.len(),
                arguments.len()
            ));
        }
        for (argument, parameter) in arguments.iter().zip(&target.arguments) {
            if !self
                .type_of(*argument)?
                .same(&Resolution::Handle(parameter.ty), &module.types)
            {
                return fail(format!(
                    "an argument of `{}` has the wrong type",
                    target.name
                ));
            }
        }
        self.flush();
        let result = target.result.as_ref().map(|result| {
            let handle = self
                .function
                .expressions
                .append(Expression::CallResult(callee), span);
            self.resolved.push(Resolution::Handle(result.ty));
            self.emit_start = handle.raw() + 1;
            handle
        });
        self.block()?.push(
            Statement::Call {
                function: callee,
                arguments,
                result,
            },
            span,
        );
        Ok(result)
    }

    /// Appends an atomic operation and returns its result when wanted.
    ///
    /// # Errors
    /// Fails when `pointer` is not a pointer to an atomic.
    #[allow(clippy::too_many_arguments)]
    pub fn atomic(
        &mut self,
        module: &mut Module,
        pointer: Handle<Expression>,
        fun: crate::ir::AtomicFunction,
        value: Handle<Expression>,
        compare: Option<Handle<Expression>>,
        want_result: bool,
        span: Span,
    ) -> Result<Option<Handle<Expression>>> {
        let scalar = match self.type_of(pointer)?.inner(&module.types) {
            TypeInner::Pointer { base, .. } => match module.types[*base].inner {
                TypeInner::Atomic(scalar) => scalar,
                _ => return fail("an atomic operation on a pointer to a non-atomic"),
            },
            _ => return fail("an atomic operation on a non-pointer"),
        };
        self.type_of(value)?;
        self.flush();
        let result = if want_result {
            let ty = module.types.insert(
                Type {
                    name: None,
                    inner: TypeInner::Scalar(scalar),
                },
                Span::default(),
            );
            let handle = self
                .function
                .expressions
                .append(Expression::AtomicResult { ty }, span);
            self.resolved.push(Resolution::Handle(ty));
            self.emit_start = handle.raw() + 1;
            Some(handle)
        } else {
            None
        };
        self.block()?.push(
            Statement::Atomic {
                pointer,
                fun,
                value,
                compare,
                result,
            },
            span,
        );
        Ok(result)
    }

    /// # Errors
    /// Fails on a condition that is not a `bool`.
    pub fn begin_if(
        &mut self,
        module: &Module,
        condition: Handle<Expression>,
        span: Span,
    ) -> Result<()> {
        if self.type_of(condition)?.inner(&module.types) != &TypeInner::Scalar(Scalar::BOOL) {
            return fail("an `if` condition is a bool");
        }
        self.flush();
        self.block()?;
        self.stack.push(Open::If {
            condition,
            span,
            accept: Block::new(),
            reject: None,
        });
        Ok(())
    }

    /// # Errors
    /// Fails outside an `if` or after its `else`.
    pub fn begin_else(&mut self) -> Result<()> {
        self.flush();
        match self.stack.last_mut() {
            Some(Open::If {
                reject: reject @ None,
                ..
            }) => {
                *reject = Some(Block::new());
                Ok(())
            }
            _ => fail("`else` outside an `if`"),
        }
    }

    /// # Errors
    /// Fails outside an `if`.
    pub fn end_if(&mut self) -> Result<()> {
        self.flush();
        match self.stack.pop() {
            Some(Open::If {
                condition,
                span,
                accept,
                reject,
            }) => self.block()?.push(
                Statement::If {
                    condition,
                    accept,
                    reject: reject.unwrap_or_default(),
                },
                span,
            ),
            other => {
                self.stack.extend(other);
                return fail("`endIf` outside an `if`");
            }
        }
        Ok(())
    }

    /// # Errors
    /// Fails directly in a switch.
    pub fn begin_loop(&mut self, span: Span) -> Result<()> {
        self.flush();
        self.block()?;
        self.stack.push(Open::Loop {
            span,
            body: Block::new(),
            continuing: None,
        });
        Ok(())
    }

    /// # Errors
    /// Fails outside a loop body.
    pub fn begin_continuing(&mut self) -> Result<()> {
        self.flush();
        match self.stack.last_mut() {
            Some(Open::Loop {
                continuing: continuing @ None,
                ..
            }) => {
                *continuing = Some(Block::new());
                Ok(())
            }
            _ => fail("`continuing` outside a loop body"),
        }
    }

    /// # Errors
    /// Fails outside a loop.
    pub fn end_loop(&mut self, break_if: Option<Handle<Expression>>) -> Result<()> {
        self.flush();
        match self.stack.pop() {
            Some(Open::Loop {
                span,
                body,
                continuing,
            }) => self.block()?.push(
                Statement::Loop {
                    body,
                    continuing: continuing.unwrap_or_default(),
                    break_if,
                },
                span,
            ),
            other => {
                self.stack.extend(other);
                return fail("`endLoop` outside a loop");
            }
        }
        Ok(())
    }

    /// # Errors
    /// Fails directly in a switch.
    pub fn begin_switch(&mut self, selector: Handle<Expression>, span: Span) -> Result<()> {
        self.type_of(selector)?;
        self.flush();
        self.block()?;
        self.stack.push(Open::Switch {
            selector,
            span,
            cases: Vec::new(),
        });
        Ok(())
    }

    /// # Errors
    /// Fails outside a switch.
    pub fn begin_case(&mut self, value: SwitchValue, fall_through: bool) -> Result<()> {
        self.flush();
        if !matches!(self.stack.last(), Some(Open::Switch { .. })) {
            return fail("a case outside a switch");
        }
        self.stack.push(Open::Case {
            value,
            fall_through,
            body: Block::new(),
        });
        Ok(())
    }

    /// # Errors
    /// Fails outside a case.
    pub fn end_case(&mut self) -> Result<()> {
        self.flush();
        let Some(Open::Case {
            value,
            fall_through,
            body,
        }) = self.stack.pop()
        else {
            return fail("`endCase` outside a case");
        };
        match self.stack.last_mut() {
            Some(Open::Switch { cases, .. }) => {
                cases.push(SwitchCase {
                    value,
                    body,
                    fall_through,
                });
                Ok(())
            }
            _ => fail("a case outside a switch"),
        }
    }

    /// # Errors
    /// Fails outside a switch.
    pub fn end_switch(&mut self) -> Result<()> {
        self.flush();
        match self.stack.pop() {
            Some(Open::Switch {
                selector,
                span,
                cases,
            }) => {
                self.block()?
                    .push(Statement::Switch { selector, cases }, span);
                Ok(())
            }
            other => {
                self.stack.extend(other);
                fail("`endSwitch` outside a switch")
            }
        }
    }

    /// Closes the body and returns the function.
    ///
    /// # Errors
    /// Fails while a construct is still open.
    pub fn finish(mut self) -> Result<Function> {
        self.flush();
        match (self.stack.pop(), self.stack.is_empty()) {
            (Some(Open::Body(body)), true) => {
                self.function.body = body;
                Ok(self.function)
            }
            _ => fail(format!(
                "`{}` finished with an open block",
                self.function.name
            )),
        }
    }
}
