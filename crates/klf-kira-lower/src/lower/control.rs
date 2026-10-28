//! Control-flow statement lowering: `if`, `while`, and `for`.

use super::{Access, Local, Lowerer};
use crate::ty::FeType;
use klf_diag::Diagnostic;
use klf_kir::StoreMode;
use klf_kira_syntax::ast::{Block, Expr, ExprKind};
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn lower_if(&mut self, branches: &[(Expr, Block)], else_block: Option<&Block>) {
        let join = self.fb.new_block(&[]);
        let mut joined = false;
        for (cond, body) in branches {
            let Some(cond_value) = self.lower_condition(cond) else {
                return;
            };
            let then_block = self.fb.new_block(&[]);
            let next_block = self.fb.new_block(&[]);
            self.fb.conditional_branch(
                cond_value,
                then_block.clone(),
                Vec::new(),
                next_block.clone(),
                Vec::new(),
            );
            self.fb.switch_to(&then_block);
            self.reachable = true;
            self.lower_block(body);
            if self.reachable {
                self.fb.branch(join.clone(), Vec::new());
                joined = true;
            }
            self.fb.switch_to(&next_block);
            self.reachable = true;
        }
        if let Some(body) = else_block {
            self.lower_block(body);
            if self.reachable {
                self.fb.branch(join.clone(), Vec::new());
                joined = true;
            }
        } else {
            self.fb.branch(join.clone(), Vec::new());
            joined = true;
        }
        if joined {
            self.fb.switch_to(&join);
            self.reachable = true;
        } else {
            self.reachable = false;
        }
    }

    pub(crate) fn lower_while(&mut self, cond: &Expr, body: &Block) {
        let header = self.fb.new_block(&[]);
        let body_block = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());
        self.fb.switch_to(&header);
        self.reachable = true;
        let Some(cond_value) = self.lower_condition(cond) else {
            return;
        };
        self.fb.conditional_branch(
            cond_value,
            body_block.clone(),
            Vec::new(),
            exit.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&body_block);
        self.reachable = true;
        self.push_loop(header.clone(), exit.clone());
        self.lower_block(body);
        if self.reachable {
            self.fb.branch(header, Vec::new());
        }
        self.loops.pop();
        self.fb.switch_to(&exit);
        self.reachable = true;
    }

    pub(crate) fn lower_for(
        &mut self,
        var: &klf_kira_syntax::ast::Ident,
        iter: &Expr,
        body: &Block,
        span: Span,
    ) {
        if let ExprKind::Range { start, end } = &iter.kind {
            self.lower_range_for(var, start, end, body, span);
            return;
        }
        let Some((container, container_ty)) = self.lower_expr(iter) else {
            return;
        };
        // The loop holds its own count on what it walks, so the body may
        // change the source without taking the elements out from under it.
        // A place inside a parameter the body cannot write is walked where it
        // is, its elements bound as aliases.
        let shared = self.shared_root(iter);
        let container = if !shared && self.counted(&container_ty) && !self.is_temporary(&container)
        {
            self.temporary_copy(container, &container_ty)
        } else {
            container
        };
        let (element, is_list, static_len) = match container_ty {
            FeType::List(element) => (*element, true, 0u64),
            FeType::Array {
                element,
                length: Some(length),
            } => (*element, false, length),
            _ => {
                self.session.report(
                    Diagnostic::error("KLFK-E0206", "`for` requires an array")
                        .with_label(span, "not iterable"),
                );
                return;
            }
        };
        let counter = self.fb.stack_alloc(klf_kir::Type::Integer(64));
        let zero = self.fb.const_int(klf_kir::Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());
        let count = if is_list {
            self.list_count(&container)
        } else {
            self.fb
                .const_int(klf_kir::Type::Integer(64), i128::from(static_len))
        };
        let header = self.fb.new_block(&[]);
        let body_block = self.fb.new_block(&[]);
        let latch = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());
        self.fb.switch_to(&header);
        self.reachable = true;
        let index = self.fb.load(counter.clone());
        let cond = self
            .fb
            .compare(klf_kir::IntegerPredicate::SignedLt, index, count.clone());
        self.fb.conditional_branch(
            cond,
            body_block.clone(),
            Vec::new(),
            exit.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&body_block);
        self.reachable = true;
        let index = self.fb.load(counter.clone());
        let place = if is_list {
            self.list_element_addr(&container, &element, index)
        } else {
            self.fb.elem_addr(container.clone(), index)
        };
        let (value, value_ty) = self.load_place(place, element);
        self.push_scope();
        self.bind(
            var.name.clone(),
            Local {
                access: Access::Ssa(value),
                ty: value_ty,
                mutable: false,
                shared: shared.then_some(super::Shared::Alias),
            },
        );
        self.push_loop(latch.clone(), exit.clone());
        self.lower_block(body);
        self.loops.pop();
        self.pop_scope();
        if self.reachable {
            self.fb.branch(latch.clone(), Vec::new());
        }
        self.fb.switch_to(&latch);
        self.reachable = true;
        let index = self.fb.load(counter.clone());
        let one = self.fb.const_int(klf_kir::Type::Integer(64), 1);
        let next = self.fb.binary(
            klf_kir::BinaryOperator::Add,
            klf_kir::Overflow::Wrap,
            true,
            index,
            one,
        );
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(header, Vec::new());
        self.fb.switch_to(&exit);
        self.reachable = true;
    }

    fn lower_range_for(
        &mut self,
        var: &klf_kira_syntax::ast::Ident,
        start: &Expr,
        end: &Expr,
        body: &Block,
        span: Span,
    ) {
        let Some((start_value, start_ty)) = self.lower_expr(start) else {
            return;
        };
        let Some((end_value, end_ty)) = self.lower_expr(end) else {
            return;
        };
        let int = FeType::Int {
            bits: 64,
            signed: true,
        };
        if start_ty != int || end_ty != int {
            self.session.report(
                Diagnostic::error("KLFK-E0206", "range bounds must have type Int")
                    .with_label(span, "expected an Int range"),
            );
            return;
        }

        let counter = self.fb.stack_alloc(klf_kir::Type::Integer(64));
        self.fb.store(StoreMode::Init, start_value, counter.clone());
        let header = self.fb.new_block(&[]);
        let body_block = self.fb.new_block(&[]);
        let latch = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());
        self.fb.switch_to(&header);
        self.reachable = true;
        let index = self.fb.load(counter.clone());
        let cond = self
            .fb
            .compare(klf_kir::IntegerPredicate::SignedLt, index, end_value);
        self.fb.conditional_branch(
            cond,
            body_block.clone(),
            Vec::new(),
            exit.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&body_block);
        self.reachable = true;
        let value = self.fb.load(counter.clone());
        self.push_scope();
        self.bind(
            var.name.clone(),
            Local {
                access: Access::Ssa(value),
                ty: int,
                mutable: false,
                shared: None,
            },
        );
        self.push_loop(latch.clone(), exit.clone());
        self.lower_block(body);
        self.loops.pop();
        self.pop_scope();
        if self.reachable {
            self.fb.branch(latch.clone(), Vec::new());
        }
        self.fb.switch_to(&latch);
        self.reachable = true;
        let index = self.fb.load(counter.clone());
        let one = self.fb.const_int(klf_kir::Type::Integer(64), 1);
        let next = self.fb.binary(
            klf_kir::BinaryOperator::Add,
            klf_kir::Overflow::Wrap,
            true,
            index,
            one,
        );
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(header, Vec::new());
        self.fb.switch_to(&exit);
        self.reachable = true;
    }
}
