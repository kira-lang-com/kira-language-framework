//! Function bodies: statements, with each expression printed where it is
//! evaluated.

use crate::Span;
use crate::arena::Handle;
use crate::ir::{Block, Expression, Statement, SwitchValue};
use crate::module::Function;
use crate::visit::statement_operands;
use std::fmt::Write;

use super::Printer;

pub(super) struct BodyPrinter<'a, 'b> {
    pub(super) printer: &'b Printer<'a>,
    pub(super) function: &'b Function,
    pub(super) names: Vec<String>,
    pub(super) next: u32,
    pub(super) out: String,
}

impl BodyPrinter<'_, '_> {
    fn indent(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str("  ");
        }
    }

    fn line(&mut self, handle: u32, depth: usize) {
        let expression = &self.function.expressions[Handle::new(handle)];
        let text = self.printer.expression(expression, Some(&self.names));
        let loc = self
            .printer
            .loc(self.function.expressions.span(Handle::new(handle)));
        self.indent(depth);
        let _ = writeln!(self.out, "%{handle} = {text}{loc}");
    }

    /// Prints the standalone expressions before `until`.
    pub(super) fn pending(&mut self, until: u32, depth: usize) {
        while self.next < until {
            let handle = self.next;
            self.next += 1;
            if !self.function.expressions[Handle::new(handle)].needs_emit() {
                self.line(handle, depth);
            }
        }
    }

    pub(super) fn block(&mut self, block: &Block, depth: usize) {
        for (statement, span) in block.iter() {
            self.statement(statement, span, depth);
        }
    }

    fn nested(&mut self, open: &str, block: &Block, depth: usize) {
        let _ = writeln!(self.out, "{open}{{");
        self.block(block, depth + 1);
        self.indent(depth);
        self.out.push('}');
    }

    fn statement(&mut self, statement: &Statement, span: Span, depth: usize) {
        let first = match statement {
            Statement::Emit(range) => Some(range.start),
            Statement::Call {
                result: Some(result),
                ..
            }
            | Statement::Atomic {
                result: Some(result),
                ..
            }
            | Statement::WorkgroupUniformLoad { result, .. } => Some(result.raw()),
            _ => None,
        };
        // A standalone operand (an argument, a local, a literal) is printed
        // before the statement that reads it.
        let read = match statement {
            Statement::Loop { .. } => None,
            Statement::Call { result, .. } | Statement::Atomic { result, .. } => {
                statement_operands(statement)
                    .into_iter()
                    .filter(|handle| Some(*handle) != *result)
                    .map(|handle| handle.raw() + 1)
                    .max()
            }
            Statement::WorkgroupUniformLoad { result, .. } => statement_operands(statement)
                .into_iter()
                .filter(|handle| handle != result)
                .map(|handle| handle.raw() + 1)
                .max(),
            _ => statement_operands(statement)
                .into_iter()
                .map(|handle| handle.raw() + 1)
                .max(),
        };
        if let Some(until) = first.into_iter().chain(read).max() {
            self.pending(until, depth);
        }
        let e = |handle: &Handle<Expression>| format!("%{}", handle.raw());
        let loc = self.printer.loc(span);
        self.indent(depth);
        match statement {
            Statement::Emit(range) => {
                self.out.push_str("emit {\n");
                for handle in range.iter() {
                    self.line(handle.raw(), depth + 1);
                }
                self.next = self.next.max(range.end);
                self.indent(depth);
                self.out.push('}');
            }
            Statement::Block(block) => self.nested("block ", block, depth),
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                self.nested(&format!("if {} ", e(condition)), accept, depth);
                if !reject.is_empty() {
                    self.nested(" else ", reject, depth);
                }
            }
            Statement::Switch { selector, cases } => {
                let _ = writeln!(self.out, "switch {} {{", e(selector));
                for case in cases {
                    self.indent(depth + 1);
                    let value = match case.value {
                        SwitchValue::I32(value) => format!("case {value}i32"),
                        SwitchValue::U32(value) => format!("case {value}u32"),
                        SwitchValue::Default => "default".to_owned(),
                    };
                    let fall = if case.fall_through {
                        " fallthrough"
                    } else {
                        ""
                    };
                    self.nested(&format!("{value}{fall}: "), &case.body, depth + 1);
                    self.out.push('\n');
                }
                self.indent(depth);
                self.out.push('}');
            }
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => {
                self.nested("loop ", body, depth);
                self.nested(" continuing ", continuing, depth);
                if let Some(condition) = break_if {
                    let _ = write!(self.out, " break_if {}", e(condition));
                }
            }
            Statement::Break => self.out.push_str("break"),
            Statement::Continue => self.out.push_str("continue"),
            Statement::Return { value: Some(value) } => {
                let _ = write!(self.out, "return {}", e(value));
            }
            Statement::Return { value: None } => self.out.push_str("return"),
            Statement::Kill => self.out.push_str("kill"),
            Statement::Store { pointer, value } => {
                let _ = write!(self.out, "store {}, {}", e(pointer), e(value));
            }
            Statement::ImageStore {
                image,
                coordinate,
                array_index,
                value,
            } => {
                let _ = write!(
                    self.out,
                    "image_store {}, {}, {}",
                    e(image),
                    e(coordinate),
                    e(value)
                );
                if let Some(index) = array_index {
                    let _ = write!(self.out, ", array_index({})", e(index));
                }
            }
            Statement::Atomic {
                pointer,
                fun,
                value,
                compare,
                result,
            } => {
                let _ = write!(
                    self.out,
                    "atomic {} {}, {}",
                    fun.name(),
                    e(pointer),
                    e(value)
                );
                if let Some(compare) = compare {
                    let _ = write!(self.out, ", {}", e(compare));
                }
                if let Some(result) = result {
                    let _ = write!(self.out, " -> {}", e(result));
                    self.next = self.next.max(result.raw() + 1);
                }
            }
            Statement::Call {
                function,
                arguments,
                result,
            } => {
                let arguments: Vec<String> = arguments.iter().map(e).collect();
                let _ = write!(
                    self.out,
                    "call @{}({})",
                    self.printer.module.functions[*function].name,
                    arguments.join(", ")
                );
                if let Some(result) = result {
                    let _ = write!(self.out, " -> {}", e(result));
                    self.next = self.next.max(result.raw() + 1);
                }
            }
            Statement::ControlBarrier(flags) => {
                let _ = write!(self.out, "control_barrier({})", flags.words().join(", "));
            }
            Statement::MemoryBarrier(flags) => {
                let _ = write!(self.out, "memory_barrier({})", flags.words().join(", "));
            }
            Statement::WorkgroupUniformLoad { pointer, result } => {
                let _ = write!(
                    self.out,
                    "workgroup_uniform_load {} -> {}",
                    e(pointer),
                    e(result)
                );
                self.next = self.next.max(result.raw() + 1);
            }
        }
        let _ = writeln!(self.out, "{loc}");
    }
}
