use super::*;

impl Lowerer<'_> {
    fn lower_field_assign(
        &mut self,
        base: &Expr,
        field: &klf_kira_syntax::ast::Ident,
        op: klf_kira_syntax::ast::AssignOp,
        value: &Expr,
        span: klf_source::Span,
    ) {
        let Some((address, field_ty)) = self.field_place_for(base, field, span, true) else {
            return;
        };
        self.store_to_place(address, &field_ty, op, value);
    }

    fn store_to_place(
        &mut self,
        address: Name,
        field_ty: &FeType,
        op: klf_kira_syntax::ast::AssignOp,
        value: &Expr,
    ) {
        let Some((rhs, rty)) = self.lower_expr_expected(value, Some(field_ty)) else {
            return;
        };
        if !field_ty.compatible(&rty) {
            self.session.report(type_error(
                value.span,
                format!(
                    "assigning {} to a field of type {}",
                    rty.display(),
                    field_ty.display()
                ),
            ));
            return;
        }
        let stored = if let Some(binop) = compound_binop(op) {
            let current = self.fb.load(address.clone());
            match self.emit_arith_binop(binop, current, rhs, field_ty, value.span) {
                Some(v) => v,
                None => return,
            }
        } else {
            rhs
        };
        self.store_value(StoreMode::Assign, stored, field_ty, address);
    }

    pub(super) fn lower_assign(
        &mut self,
        target: &Expr,
        op: klf_kira_syntax::ast::AssignOp,
        value: &Expr,
    ) {
        if let ExprKind::Field { base, name } = &target.kind {
            self.lower_field_assign(base, name, op, value, target.span);
            return;
        }
        if let ExprKind::Index { base, index } = &target.kind {
            let Some((address, element_ty)) = self.index_place_for(base, index, target.span, true)
            else {
                return;
            };
            self.store_to_place(address, &element_ty, op, value);
            return;
        }
        if let ExprKind::Path(path) = &target.kind
            && (path.segments.len() > 1 || self.implicit_self(path).is_some())
        {
            let Some((address, field_ty)) = self.path_place(path, target.span) else {
                return;
            };
            self.store_to_place(address, &field_ty, op, value);
            return;
        }
        let ExprKind::Path(path) = &target.kind else {
            self.session.report(
                Diagnostic::error("KLFK-E0206", "assignment target is outside the M4 subset")
                    .with_label(target.span, "expected a variable"),
            );
            return;
        };
        let name = &path.segments[0].name;
        let (slot, ty, mutable) = match self.lookup(name) {
            Some(Local {
                access: Access::Slot(slot),
                ty,
                mutable,
                ..
            }) => (slot.clone(), ty.clone(), *mutable),
            Some(Local {
                access: Access::Ssa(_),
                ty,
                ..
            }) => (Name::new(""), ty.clone(), false),
            None => {
                self.session.report(
                    Diagnostic::error("KLFK-E0200", format!("unknown name `{name}`"))
                        .with_label(target.span, "not found"),
                );
                return;
            }
        };
        if !mutable {
            self.session.report(
                Diagnostic::error("KLFK-E0208", format!("cannot assign to immutable `{name}`"))
                    .with_label(target.span, "declared with `let`"),
            );
            return;
        }
        let Some((rhs, rty)) = self.lower_expr_expected(value, Some(&ty)) else {
            return;
        };
        if !ty.compatible(&rty) {
            self.session.report(type_error(
                value.span,
                format!(
                    "assigning {} to a variable of type {}",
                    rty.display(),
                    ty.display()
                ),
            ));
            return;
        }
        // An aggregate `var` keeps its storage; assigning copies into it.
        if lists::is_aggregate(&ty) {
            let storage = self.fb.load(slot);
            self.store_value(StoreMode::Assign, rhs, &ty, storage);
            return;
        }
        let stored = if let Some(binop) = compound_binop(op) {
            let current = self.fb.load(slot.clone());
            match self.emit_arith_binop(binop, current, rhs, &ty, value.span) {
                Some(v) => v,
                None => return,
            }
        } else {
            rhs
        };
        self.fb.store(StoreMode::Assign, stored, slot);
    }

    /// Lowers a test, releasing what it built before control branches on it.
    pub(super) fn lower_condition(&mut self, cond: &Expr) -> Option<Name> {
        self.open_region();
        let value = self.lower_condition_value(cond);
        self.close_region();
        value
    }

    fn lower_condition_value(&mut self, cond: &Expr) -> Option<Name> {
        let (value, ty) = self.lower_expr(cond)?;
        if ty != FeType::Bool {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0204",
                    format!("condition has type {}, expected Bool", ty.display()),
                )
                .with_label(cond.span, "not a Bool"),
            );
            return None;
        }
        Some(value)
    }

    pub(super) fn lower_return(&mut self, value: Option<&Expr>, span: klf_source::Span) {
        if let Some(expr) = value {
            let expected = self.result_ty.clone();
            if let Some((v, vty)) = self.lower_expr_expected(expr, Some(&expected)) {
                if vty != self.result_ty {
                    self.session.report(
                        Diagnostic::error(
                            "KLFK-E0205",
                            format!(
                                "returning {} from a function returning {}",
                                vty.display(),
                                self.result_ty.display()
                            ),
                        )
                        .with_label(span, "type mismatch"),
                    );
                }
                if let Some(session) = self.definition_session.clone() {
                    self.return_unit_list(&session, &v);
                } else if let Some(destination) = self.ret_slot.clone() {
                    let result_ty = self.result_ty.clone();
                    self.init_from(v, &result_ty, destination);
                    self.release_all();
                    self.fb.ret(None);
                } else {
                    self.release_all();
                    self.fb.ret(Some(v));
                }
            }
        } else {
            if self.result_ty != FeType::Void {
                self.session.report(
                    Diagnostic::error(
                        "KLFK-E0205",
                        format!(
                            "empty return from a function returning {}",
                            self.result_ty.display()
                        ),
                    )
                    .with_label(span, "expected a value"),
                );
            }
            self.release_all();
            self.fb.ret(None);
        }
        self.reachable = false;
    }

    pub(super) fn lower_jump(&mut self, is_break: bool, span: klf_source::Span) {
        let Some(ctx) = self.loops.last() else {
            self.session.report(
                Diagnostic::error("KLFK-E0206", "`break`/`continue` outside a loop")
                    .with_label(span, "not in a loop"),
            );
            return;
        };
        let target = if is_break {
            ctx.break_block.clone()
        } else {
            ctx.continue_block.clone()
        };
        let (regions, scopes) = (ctx.regions, ctx.scopes);
        self.release_since(regions, scopes);
        self.fb.branch(target, Vec::new());
        self.reachable = false;
    }
}
fn compound_binop(op: klf_kira_syntax::ast::AssignOp) -> Option<klf_kira_syntax::ast::BinOp> {
    use klf_kira_syntax::ast::{AssignOp, BinOp};
    let binop = match op {
        AssignOp::Assign => return None,
        AssignOp::Add => BinOp::Add,
        AssignOp::Sub => BinOp::Sub,
        AssignOp::Mul => BinOp::Mul,
        AssignOp::Div => BinOp::Div,
        AssignOp::Rem => BinOp::Rem,
        AssignOp::And => BinOp::BitAnd,
        AssignOp::Or => BinOp::BitOr,
        AssignOp::Xor => BinOp::BitXor,
        AssignOp::Shl => BinOp::Shl,
        AssignOp::Shr => BinOp::Shr,
    };
    Some(binop)
}
