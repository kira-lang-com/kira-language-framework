//! Function bodies and expressions of textual KSIR.

use super::Parser;
use super::lexer::Token;
use crate::Span;
use crate::arena::{Handle, Range};
use crate::ir::{
    AtomicFunction, BarrierFlags, Block, Expression, Statement, SwitchCase, SwitchValue,
};
use crate::module::{Function, FunctionArgument, FunctionResult, LocalVariable};
use std::collections::HashMap;

/// What names inside a function body resolve against.
pub(crate) struct Scope {
    pub(super) locals: HashMap<String, u32>,
    pub(super) count: u32,
}

impl Parser {
    pub(super) fn body(
        &mut self,
        name: String,
        arguments: Vec<FunctionArgument>,
        result: Option<FunctionResult>,
    ) -> Result<(Function, Span), String> {
        self.expect_punct('{')?;
        let span = self.loc()?;
        let mut function = Function {
            name,
            arguments,
            result,
            ..Function::default()
        };
        let mut scope = Scope {
            locals: HashMap::new(),
            count: 0,
        };
        while self.is_word("local") {
            self.at += 1;
            self.expect_punct_dollar()?;
            let printed = self.ident()?;
            self.expect_punct(':')?;
            let ty = self.ty()?;
            let init = if self.eat_punct('=') {
                self.expect_punct('%')?;
                Some(Handle::new(self.number()?))
            } else {
                None
            };
            let local_span = self.loc()?;
            let name = local_source_name(&printed);
            let handle = function
                .locals
                .append(LocalVariable { name, ty, init }, local_span);
            scope.locals.insert(printed, handle.raw());
        }
        function.body = self.block_lines(&mut function, &mut scope)?;
        Ok((function, span))
    }

    pub(super) fn expect_punct_dollar(&mut self) -> Result<(), String> {
        match self.next()? {
            Token::Dollar => Ok(()),
            other => self.error(format!("expected `$`, found {other:?}")),
        }
    }

    pub(super) fn local_ref(&mut self, scope: &Scope) -> Result<Handle<Expression>, String> {
        self.expect_punct('%')?;
        let number = self.number()?;
        if number >= scope.count {
            return self.error(format!("`%{number}` is not defined yet"));
        }
        Ok(Handle::new(number))
    }

    /// Lines up to the closing `}` of the current block.
    pub(super) fn block_lines(
        &mut self,
        function: &mut Function,
        scope: &mut Scope,
    ) -> Result<Block, String> {
        let mut block = Block::new();
        while !self.eat_punct('}') {
            if self.is_punct('%') {
                self.define(function, scope)?;
                continue;
            }
            let (statement, span) = self.statement(function, scope)?;
            block.push(statement, span);
        }
        Ok(block)
    }

    /// `%N = expr`, appended to the function's arena.
    pub(super) fn define(
        &mut self,
        function: &mut Function,
        scope: &mut Scope,
    ) -> Result<Handle<Expression>, String> {
        self.expect_punct('%')?;
        let number = self.number()?;
        if number != scope.count {
            return self.error(format!("expected `%{}`, found `%{number}`", scope.count));
        }
        self.expect_punct('=')?;
        let expression = self.expression(Some(scope))?;
        let span = self.loc()?;
        scope.count += 1;
        Ok(function.expressions.append(expression, span))
    }

    pub(super) fn nested(
        &mut self,
        function: &mut Function,
        scope: &mut Scope,
    ) -> Result<Block, String> {
        self.expect_punct('{')?;
        self.block_lines(function, scope)
    }

    pub(super) fn statement(
        &mut self,
        function: &mut Function,
        scope: &mut Scope,
    ) -> Result<(Statement, Span), String> {
        let word = self.ident()?;
        let statement = match word.as_str() {
            "emit" => {
                self.expect_punct('{')?;
                let start = scope.count;
                while !self.eat_punct('}') {
                    self.define(function, scope)?;
                }
                Statement::Emit(Range::new(start, scope.count))
            }
            "block" => Statement::Block(self.nested(function, scope)?),
            "if" => {
                let condition = self.local_ref(scope)?;
                let accept = self.nested(function, scope)?;
                let reject = if self.eat_word("else") {
                    self.nested(function, scope)?
                } else {
                    Block::new()
                };
                Statement::If {
                    condition,
                    accept,
                    reject,
                }
            }
            "switch" => {
                let selector = self.local_ref(scope)?;
                self.expect_punct('{')?;
                let mut cases = Vec::new();
                while !self.eat_punct('}') {
                    let value = if self.eat_word("default") {
                        SwitchValue::Default
                    } else {
                        self.expect_word("case")?;
                        match self.literal()? {
                            crate::ir::Literal::I32(value) => SwitchValue::I32(value),
                            crate::ir::Literal::U32(value) => SwitchValue::U32(value),
                            _ => return self.error("a case value is an i32 or u32"),
                        }
                    };
                    let fall_through = self.eat_word("fallthrough");
                    self.expect_punct(':')?;
                    let body = self.nested(function, scope)?;
                    cases.push(SwitchCase {
                        value,
                        body,
                        fall_through,
                    });
                }
                Statement::Switch { selector, cases }
            }
            "loop" => {
                let body = self.nested(function, scope)?;
                self.expect_word("continuing")?;
                let continuing = self.nested(function, scope)?;
                let break_if = if self.eat_word("break_if") {
                    Some(self.local_ref(scope)?)
                } else {
                    None
                };
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                }
            }
            "break" => Statement::Break,
            "continue" => Statement::Continue,
            "kill" => Statement::Kill,
            "return" => {
                let value = if self.is_punct('%') {
                    Some(self.local_ref(scope)?)
                } else {
                    None
                };
                Statement::Return { value }
            }
            "store" => {
                let pointer = self.local_ref(scope)?;
                self.expect_punct(',')?;
                Statement::Store {
                    pointer,
                    value: self.local_ref(scope)?,
                }
            }
            "image_store" => {
                let image = self.local_ref(scope)?;
                self.expect_punct(',')?;
                let coordinate = self.local_ref(scope)?;
                self.expect_punct(',')?;
                let value = self.local_ref(scope)?;
                let mut array_index = None;
                if self.eat_punct(',') {
                    self.expect_word("array_index")?;
                    array_index = Some(self.named_ref(scope)?);
                }
                Statement::ImageStore {
                    image,
                    coordinate,
                    array_index,
                    value,
                }
            }
            "atomic" => {
                let fun = self.ident()?;
                let fun = AtomicFunction::from_name(&fun)
                    .map_or_else(|| self.error("unknown atomic function"), Ok)?;
                let pointer = self.local_ref(scope)?;
                self.expect_punct(',')?;
                let value = self.local_ref(scope)?;
                let compare = if self.eat_punct(',') {
                    Some(self.local_ref(scope)?)
                } else {
                    None
                };
                let result = self.result_ref(function, scope, pointer, |ty| {
                    Expression::AtomicResult { ty }
                })?;
                Statement::Atomic {
                    pointer,
                    fun,
                    value,
                    compare,
                    result,
                }
            }
            "call" => {
                let callee = self.at_name()?;
                let callee = self
                    .module
                    .functions
                    .iter()
                    .find(|(_, function)| function.name == callee)
                    .map(|(handle, _)| handle)
                    .map_or_else(|| self.error(format!("unknown function `@{callee}`")), Ok)?;
                self.expect_punct('(')?;
                let mut arguments = Vec::new();
                while !self.eat_punct(')') {
                    arguments.push(self.local_ref(scope)?);
                    self.eat_punct(',');
                }
                let result = if self.peek() == Some(&Token::Arrow) {
                    self.at += 1;
                    let number = self.expect_next_number()?;
                    if number != scope.count {
                        return self.error("a call result takes the next expression number");
                    }
                    scope.count += 1;
                    Some(
                        function
                            .expressions
                            .append(Expression::CallResult(callee), Span::default()),
                    )
                } else {
                    None
                };
                Statement::Call {
                    function: callee,
                    arguments,
                    result,
                }
            }
            "control_barrier" | "memory_barrier" => {
                self.expect_punct('(')?;
                let mut flags = BarrierFlags::default();
                while !self.eat_punct(')') {
                    match self.ident()?.as_str() {
                        "storage" => flags.storage = true,
                        "workgroup" => flags.workgroup = true,
                        "image" => flags.image = true,
                        other => return self.error(format!("unknown barrier flag `{other}`")),
                    }
                    self.eat_punct(',');
                }
                if word == "control_barrier" {
                    Statement::ControlBarrier(flags)
                } else {
                    Statement::MemoryBarrier(flags)
                }
            }
            "workgroup_uniform_load" => {
                let pointer = self.local_ref(scope)?;
                let result = self
                    .result_ref(function, scope, pointer, |ty| {
                        Expression::WorkgroupUniformLoadResult { ty }
                    })?
                    .map_or_else(|| self.error("workgroup_uniform_load names its result"), Ok)?;
                Statement::WorkgroupUniformLoad { pointer, result }
            }
            other => return self.error(format!("unknown statement `{other}`")),
        };
        let span = self.loc()?;
        Ok((statement, span))
    }

    pub(super) fn expect_next_number(&mut self) -> Result<u32, String> {
        self.expect_punct('%')?;
        self.number()
    }

    /// An optional `-> %N` result, typed by the value `pointer` points at.
    pub(super) fn result_ref(
        &mut self,
        function: &mut Function,
        scope: &mut Scope,
        pointer: Handle<Expression>,
        make: impl Fn(crate::arena::Handle<crate::types::Type>) -> Expression,
    ) -> Result<Option<Handle<Expression>>, String> {
        if self.peek() != Some(&Token::Arrow) {
            return Ok(None);
        }
        self.at += 1;
        let number = self.expect_next_number()?;
        if number != scope.count {
            return self.error("a result takes the next expression number");
        }
        let resolved = crate::resolve::resolve_function(&self.module, function)
            .map_err(|(_, message)| message)?;
        let Some(crate::types::TypeInner::Pointer { base: target, .. }) = resolved
            .get(pointer.index())
            .map(|operand| operand.inner(&self.module.types).clone())
        else {
            return self.error("the result's operand is not a pointer");
        };
        let ty = match self.module.types[target].inner {
            crate::types::TypeInner::Atomic(scalar) => {
                self.intern(crate::types::TypeInner::Scalar(scalar))
            }
            _ => target,
        };
        scope.count += 1;
        Ok(Some(function.expressions.append(make(ty), Span::default())))
    }

    pub(super) fn named_ref(&mut self, scope: &Scope) -> Result<Handle<Expression>, String> {
        self.expect_punct('(')?;
        let handle = self.local_ref(scope)?;
        self.expect_punct(')')?;
        Ok(handle)
    }

    pub(super) fn operand(&mut self, scope: Option<&Scope>) -> Result<Handle<Expression>, String> {
        match scope {
            Some(scope) => self.local_ref(scope),
            None => self.global_ref_any(),
        }
    }

    pub(super) fn global_ref_any(&mut self) -> Result<Handle<Expression>, String> {
        self.expect_punct('%')?;
        let word = self.ident()?;
        let index: u32 = word
            .strip_prefix('g')
            .and_then(|digits| digits.parse().ok())
            .map_or_else(|| self.error("expected `%gN`"), Ok)?;
        Ok(Handle::new(index))
    }

    pub(super) fn operand_list(
        &mut self,
        scope: Option<&Scope>,
    ) -> Result<Vec<Handle<Expression>>, String> {
        let mut operands = vec![self.operand(scope)?];
        while self.eat_punct(',') {
            operands.push(self.operand(scope)?);
        }
        Ok(operands)
    }
}

/// The source name of a printed local name: `a.2` is `a`, `_7` is none.
fn local_source_name(printed: &str) -> Option<String> {
    if printed
        .strip_prefix('_')
        .is_some_and(|rest| rest.parse::<u32>().is_ok())
    {
        return None;
    }
    Some(printed.split('.').next().unwrap_or(printed).to_owned())
}
