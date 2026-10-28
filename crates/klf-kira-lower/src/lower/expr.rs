//! Expression lowering for the M4 subset.

use super::Access;
use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{
    BinaryOperator, ConvertKind, FloatBinaryOperator, FloatPredicate, FloatUnaryOperator,
    IntegerPredicate, Overflow,
};
use klf_kira_syntax::ast::{BinOp, Expr, ExprKind, Ident, UnaryOp};
use klf_source::Span;
use std::cmp::Ordering;

impl Lowerer<'_> {
    /// Lowers an expression, returning its SSA value and frontend type. On a
    /// type or resolution error it reports a diagnostic and returns `None`.
    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> Option<(Name, FeType)> {
        // An expression places what it builds at its own span, and puts the
        // enclosing expression's location back before returning, so an operand
        // does not claim the operation its parent emits.
        let enclosing = self.fb.location().cloned();
        self.at(expr.span);
        let value = self.lower_expr_kind(expr);
        self.fb.at(enclosing);
        value
    }

    fn lower_expr_kind(&mut self, expr: &Expr) -> Option<(Name, FeType)> {
        match &expr.kind {
            ExprKind::Int(text) => {
                let value = parse_int(text.as_str())?;
                let ty = FeType::Int {
                    bits: 64,
                    signed: true,
                };
                Some((self.fb.const_int(ty.to_kir(), value), ty))
            }
            ExprKind::Float(text) => {
                let literal = text
                    .as_str()
                    .chars()
                    .filter(|c| *c != '_')
                    .collect::<String>();
                if literal.parse::<f64>().is_err() {
                    self.session
                        .report(type_error(expr.span, "invalid float literal"));
                    return None;
                }
                let ty = FeType::Float { bits: 64 };
                Some((self.fb.const_float(ty.to_kir(), literal), ty))
            }
            ExprKind::Str(value) => self.lower_string_lit(value, expr.span),
            ExprKind::Try(operand) => self.lower_try(operand, expr.span),
            ExprKind::Bool(value) => Some((self.fb.const_bool(*value), FeType::Bool)),
            ExprKind::Path(path) => self.lower_path(path, expr.span),
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, expr.span),
            ExprKind::Binary { op, lhs, rhs } => self.lower_binary(*op, lhs, rhs, expr.span),
            ExprKind::As { expr, ty } => self.lower_cast(expr, ty, expr.span),
            ExprKind::Is { .. } => {
                self.session.report(
                    Diagnostic::error("KLFK-E0206", "`is` expressions are not lowered yet")
                        .with_label(expr.span, "not supported yet"),
                );
                None
            }
            ExprKind::Call { callee, args } => self.lower_call(callee, args, expr.span, None),
            ExprKind::Ternary {
                cond,
                then,
                otherwise,
            } => self.lower_ternary(cond, then, otherwise),
            ExprKind::Range { .. } => {
                self.session.report(
                    Diagnostic::error("KLFK-E0206", "ranges are only valid as `for` iterators")
                        .with_label(expr.span, "not a value expression"),
                );
                None
            }
            ExprKind::EnumLit { name, args } => {
                self.lower_enum_lit(name, args.as_deref(), None, expr.span)
            }
            ExprKind::StructLit { ty, fields } => self.lower_struct_lit(ty, fields, expr.span),
            ExprKind::ArrayLit(items) => self.lower_list_lit(items, None, expr.span),
            ExprKind::Field { base, name } => {
                let (address, field_ty) = self.field_place(base, name, expr.span)?;
                Some(self.load_place(address, field_ty))
            }
            ExprKind::Index { base, index } => {
                let (address, element_ty) = self.index_place(base, index, expr.span)?;
                Some(self.load_place(address, element_ty))
            }
            _ => {
                self.session.report(
                    Diagnostic::error("KLFK-E0206", "this expression is outside the M4 subset")
                        .with_label(expr.span, "not lowered yet"),
                );
                None
            }
        }
    }

    /// Lower an expression, passing an expected type down so an empty array
    /// literal `[]` can take its element type from context.
    pub(crate) fn lower_expr_expected(
        &mut self,
        expr: &Expr,
        expected: Option<&FeType>,
    ) -> Option<(Name, FeType)> {
        if let ExprKind::ArrayLit(items) = &expr.kind {
            return self.lower_list_lit(items, expected, expr.span);
        }
        if let ExprKind::EnumLit { name, args } = &expr.kind {
            return self.lower_enum_lit(name, args.as_deref(), expected, expr.span);
        }
        if let ExprKind::Closure { params, body } = &expr.kind {
            return self.lower_closure(params, body, expected, expr.span);
        }
        let artifact = self.try_artifact_value(expr, expected);
        if !matches!(artifact, Builtin::Skipped) {
            return artifact.into_value();
        }
        if let ExprKind::Call { callee, args } = &expr.kind {
            return self.lower_call(callee, args, expr.span, expected);
        }
        self.lower_expr(expr)
    }

    pub(super) fn lower_path(
        &mut self,
        path: &klf_kira_syntax::ast::Path,
        span: Span,
    ) -> Option<(Name, FeType)> {
        if path.segments.len() > 1 {
            let (address, ty) = self.path_place(path, span)?;
            return Some(self.load_place(address, ty));
        }
        if let Some(path) = self.implicit_self(path) {
            let (address, ty) = self.path_place(&path, span)?;
            return Some(self.load_place(address, ty));
        }
        let name = &path.segments[0].name;
        let Some(local) = self.lookup(name) else {
            if let Some(constant) = self.constants.get(name) {
                return self.lower_expr(&constant.value);
            }
            self.session.report(
                Diagnostic::error("KLFK-E0200", format!("unknown name `{name}`"))
                    .with_label(span, "not found"),
            );
            return None;
        };
        let ty = local.ty.clone();
        let access = match &local.access {
            Access::Ssa(value) => Access::Ssa(value.clone()),
            Access::Slot(slot) => Access::Slot(slot.clone()),
        };
        let value = match access {
            Access::Ssa(value) => value,
            Access::Slot(slot) => self.fb.load(slot),
        };
        Some((value, ty))
    }

    /// `move place`: the value with the counts the place held, the place
    /// left empty. Moving anything but a place is the value itself.
    fn lower_move(&mut self, operand: &Expr) -> Option<(Name, FeType)> {
        // Storage the function does not own stays as it is: a move out of
        // it hands over a copy.
        if self.shared_root(operand) {
            let (value, ty) = self.lower_expr(operand)?;
            if !self.counted(&ty) {
                return Some((value, ty));
            }
            return Some((self.temporary_copy(value, &ty), ty));
        }
        let (place, ty) = match &operand.kind {
            ExprKind::Path(path)
                if self.lookup(&path.segments[0].name).is_some()
                    || self.implicit_self(path).is_some() =>
            {
                self.path_place(path, operand.span)?
            }
            ExprKind::Field { base, name } => self.field_place(base, name, operand.span)?,
            _ => return self.lower_expr(operand),
        };
        if !super::lists::is_aggregate(&ty) {
            return Some(self.load_place(place, ty));
        }
        Some(self.move_out(place, &ty))
    }

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> Option<(Name, FeType)> {
        if op == UnaryOp::Move {
            return self.lower_move(operand);
        }
        if op == UnaryOp::Copy {
            return self.lower_expr(operand);
        }
        let (value, ty) = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty.is_float() {
                    let result = self.fb.float_unary(FloatUnaryOperator::Neg, value);
                    return Some((result, ty));
                }
                let FeType::Int { .. } = ty else {
                    self.session
                        .report(type_error(span, "`-` applies to a number"));
                    return None;
                };
                let zero = self.fb.const_int(ty.to_kir(), 0);
                let result = self.fb.binary(
                    BinaryOperator::Sub,
                    Overflow::Trap,
                    ty.is_signed(),
                    zero,
                    value,
                );
                Some((result, ty))
            }
            UnaryOp::Not => {
                if ty != FeType::Bool {
                    self.session
                        .report(type_error(span, "`!` applies to a Bool"));
                    return None;
                }
                let ones = self.fb.const_bool(true);
                let result =
                    self.fb
                        .binary(BinaryOperator::Xor, Overflow::Wrap, false, value, ones);
                Some((result, FeType::Bool))
            }
            UnaryOp::BitNot => {
                let FeType::Int { .. } = ty else {
                    self.session
                        .report(type_error(span, "`~` applies to an integer"));
                    return None;
                };
                let ones = self.fb.const_int(ty.to_kir(), -1);
                let result =
                    self.fb
                        .binary(BinaryOperator::Xor, Overflow::Wrap, false, value, ones);
                Some((result, ty))
            }
            UnaryOp::Move | UnaryOp::Copy => unreachable!("handled above"),
        }
    }

    fn lower_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> Option<(Name, FeType)> {
        if matches!(op, BinOp::And | BinOp::Or) {
            return self.lower_logical(op, lhs, rhs, span);
        }
        let ((lv, lty), (rv, rty)) = self.lower_operands(lhs, rhs)?;
        if lty != rty {
            self.session.report(type_error(
                span,
                format!(
                    "operands have different types: {} and {}",
                    lty.display(),
                    rty.display()
                ),
            ));
            return None;
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) && is_string(&lty) {
            return Some((self.lower_string_eq(lv, rv, op == BinOp::Ne), FeType::Bool));
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) && matches!(lty, FeType::Enum(_)) {
            // Enum equality compares tags, not the aggregate handles.
            let left_tag = self.fb.enum_tag(lv);
            let right_tag = self.fb.enum_tag(rv);
            let predicate = if op == BinOp::Eq {
                IntegerPredicate::Eq
            } else {
                IntegerPredicate::Ne
            };
            return Some((
                self.fb.compare(predicate, left_tag, right_tag),
                FeType::Bool,
            ));
        }
        if lty.is_float() {
            if let Some(predicate) = float_comparison(op) {
                return Some((self.fb.float_compare(predicate, lv, rv), FeType::Bool));
            }
            let result = self.emit_float_binop(op, lv, rv, span)?;
            return Some((result, lty));
        }
        if let Some(predicate) = comparison(op, lty.is_signed()) {
            return Some((self.fb.compare(predicate, lv, rv), FeType::Bool));
        }
        let result = self.emit_int_binop(op, lv, rv, &lty, span)?;
        Some((result, lty))
    }

    /// `&&` and `||` evaluate the right operand only when the left one does not
    /// decide the result. A guard such as `i < list.count && list[i] == x`
    /// depends on that: the right operand is reached only when the index is in
    /// range.
    /// Both operands, left to right. A leading-dot enum case takes its enum
    /// from the other operand, so `kind == .Less` resolves against `kind`.
    fn lower_operands(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Option<((Name, FeType), (Name, FeType))> {
        if matches!(lhs.kind, ExprKind::EnumLit { .. })
            && !matches!(rhs.kind, ExprKind::EnumLit { .. })
        {
            let right = self.lower_expr(rhs)?;
            let left = self.lower_expr_expected(lhs, Some(&right.1))?;
            return Some((left, right));
        }
        let left = self.lower_expr(lhs)?;
        let right = self.lower_expr_expected(rhs, Some(&left.1))?;
        Some((left, right))
    }

    fn lower_logical(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> Option<(Name, FeType)> {
        let (lv, lty) = self.lower_expr(lhs)?;
        if lty != FeType::Bool {
            self.session
                .report(type_error(span, "`&&`/`||` apply to Bool operands"));
            return None;
        }
        let rhs_block = self.fb.new_block(&[]);
        let result = self.fb.fresh_value();
        let join = self
            .fb
            .new_block(&[(result.clone(), FeType::Bool.to_kir())]);
        if op == BinOp::And {
            self.fb.conditional_branch(
                lv.clone(),
                rhs_block.clone(),
                Vec::new(),
                join.clone(),
                vec![lv],
            );
        } else {
            self.fb.conditional_branch(
                lv.clone(),
                join.clone(),
                vec![lv],
                rhs_block.clone(),
                Vec::new(),
            );
        }
        self.fb.switch_to(&rhs_block);
        // The right operand runs on one path only, so what it builds is
        // released on that path.
        self.open_region();
        let right = self.lower_expr(rhs);
        self.close_region();
        let (rv, rty) = right?;
        if rty != FeType::Bool {
            self.session
                .report(type_error(span, "`&&`/`||` apply to Bool operands"));
            return None;
        }
        self.fb.branch(join.clone(), vec![rv]);
        self.fb.switch_to(&join);
        Some((result, FeType::Bool))
    }

    fn lower_cast(
        &mut self,
        expr: &Expr,
        target_syntax: &klf_kira_syntax::ast::Type,
        span: Span,
    ) -> Option<(Name, FeType)> {
        let (value, source) = self.lower_expr(expr)?;
        let target = self.resolve(target_syntax)?;
        match (&source, &target) {
            (FeType::DefinitionHandle(_), FeType::Int { bits: 64, .. })
            | (FeType::Int { bits: 64, .. }, FeType::DefinitionHandle(_)) => Some((value, target)),
            (
                FeType::Int {
                    bits: source_bits,
                    signed: source_signed,
                },
                FeType::Int {
                    bits: target_bits, ..
                },
            ) => {
                let kind = match target_bits.cmp(source_bits) {
                    Ordering::Less => ConvertKind::Truncate,
                    Ordering::Greater if *source_signed => ConvertKind::SignExtend,
                    Ordering::Greater => ConvertKind::ZeroExtend,
                    Ordering::Equal => return Some((value, target)),
                };
                Some((self.fb.convert(kind, value, target.to_kir()), target))
            }
            (FeType::Int { signed, .. }, FeType::Float { .. }) => {
                let kind = if *signed {
                    ConvertKind::SignedToFloat
                } else {
                    ConvertKind::UnsignedToFloat
                };
                Some((self.fb.convert(kind, value, target.to_kir()), target))
            }
            (FeType::Float { .. }, FeType::Int { signed, .. }) => {
                let kind = if *signed {
                    ConvertKind::FloatToSigned
                } else {
                    ConvertKind::FloatToUnsigned
                };
                Some((self.fb.convert(kind, value, target.to_kir()), target))
            }
            (FeType::Float { bits: source_bits }, FeType::Float { bits: target_bits }) => {
                let kind = match target_bits.cmp(source_bits) {
                    Ordering::Less => ConvertKind::FloatTruncate,
                    Ordering::Greater => ConvertKind::FloatExtend,
                    Ordering::Equal => return Some((value, target)),
                };
                Some((self.fb.convert(kind, value, target.to_kir()), target))
            }
            _ => {
                if source == target {
                    return Some((value, target));
                }
                self.session.report(type_error(span, "unsupported cast"));
                None
            }
        }
    }

    /// Emits an integer arithmetic, bitwise or shift op (not a comparison).
    pub(crate) fn emit_int_binop(
        &mut self,
        op: BinOp,
        lhs: Name,
        rhs: Name,
        ty: &FeType,
        span: Span,
    ) -> Option<Name> {
        let signed = ty.is_signed();
        let (operator, overflow) = match op {
            BinOp::Add => (BinaryOperator::Add, Overflow::Trap),
            BinOp::Sub => (BinaryOperator::Sub, Overflow::Trap),
            BinOp::Mul => (BinaryOperator::Mul, Overflow::Trap),
            BinOp::WrapAdd => (BinaryOperator::Add, Overflow::Wrap),
            BinOp::WrapSub => (BinaryOperator::Sub, Overflow::Wrap),
            BinOp::WrapMul => (BinaryOperator::Mul, Overflow::Wrap),
            BinOp::Div => (
                if signed {
                    BinaryOperator::SignedDiv
                } else {
                    BinaryOperator::UnsignedDiv
                },
                Overflow::Wrap,
            ),
            BinOp::Rem => (
                if signed {
                    BinaryOperator::SignedRem
                } else {
                    BinaryOperator::UnsignedRem
                },
                Overflow::Wrap,
            ),
            BinOp::BitAnd => (BinaryOperator::And, Overflow::Wrap),
            BinOp::BitOr => (BinaryOperator::Or, Overflow::Wrap),
            BinOp::BitXor => (BinaryOperator::Xor, Overflow::Wrap),
            BinOp::Shl => (BinaryOperator::ShiftLeft, Overflow::Wrap),
            BinOp::Shr => (
                if signed {
                    BinaryOperator::ArithmeticShiftRight
                } else {
                    BinaryOperator::LogicalShiftRight
                },
                Overflow::Wrap,
            ),
            _ => {
                self.session
                    .report(type_error(span, "operator is outside the M4 subset"));
                return None;
            }
        };
        Some(self.fb.binary(operator, overflow, signed, lhs, rhs))
    }

    /// Emits an arithmetic op, dispatching on whether the operands are floats.
    pub(crate) fn emit_arith_binop(
        &mut self,
        op: BinOp,
        lhs: Name,
        rhs: Name,
        ty: &FeType,
        span: Span,
    ) -> Option<Name> {
        if ty.is_float() {
            self.emit_float_binop(op, lhs, rhs, span)
        } else {
            self.emit_int_binop(op, lhs, rhs, ty, span)
        }
    }

    /// Emits a floating-point arithmetic op (not a comparison).
    fn emit_float_binop(&mut self, op: BinOp, lhs: Name, rhs: Name, span: Span) -> Option<Name> {
        let operator = match op {
            BinOp::Add => FloatBinaryOperator::Add,
            BinOp::Sub => FloatBinaryOperator::Sub,
            BinOp::Mul => FloatBinaryOperator::Mul,
            BinOp::Div => FloatBinaryOperator::Div,
            BinOp::Rem => FloatBinaryOperator::Rem,
            _ => {
                self.session
                    .report(type_error(span, "operator does not apply to floats"));
                return None;
            }
        };
        Some(self.fb.float_binary(operator, lhs, rhs))
    }

    /// Lower a struct literal: allocate storage, store each field, and yield the
    /// storage address as the struct value.
    fn lower_ternary(
        &mut self,
        cond: &Expr,
        then: &Expr,
        otherwise: &Expr,
    ) -> Option<(Name, FeType)> {
        self.open_region();
        let test = self.lower_expr(cond);
        self.close_region();
        let (cond_value, cond_ty) = test?;
        if cond_ty != FeType::Bool {
            self.session
                .report(type_error(cond.span, "a conditional's test must be Bool"));
            return None;
        }
        let then_block = self.fb.new_block(&[]);
        let else_block = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            cond_value,
            then_block.clone(),
            Vec::new(),
            else_block.clone(),
            Vec::new(),
        );

        // Each arm runs on its own path and releases what it built there; an
        // arm whose value holds buffers hands on a copy of its own, which the
        // statement then owns.
        self.fb.switch_to(&then_block);
        self.open_region();
        let Some((then_value, then_ty)) = self.lower_expr(then) else {
            self.close_region();
            return None;
        };
        let counted = self.counted(&then_ty);
        let result_param = self.fb.fresh_value();
        // An aggregate arm's value is the address of its storage.
        let result_type = if super::lists::is_aggregate(&then_ty) {
            klf_kir::Type::Address(Box::new(then_ty.to_kir()))
        } else {
            then_ty.to_kir()
        };
        let join = self.fb.new_block(&[(result_param.clone(), result_type)]);
        let then_value = self.ternary_arm_value(then_value, &then_ty, counted);
        self.close_region();
        self.fb.branch(join.clone(), vec![then_value]);

        self.fb.switch_to(&else_block);
        self.open_region();
        let Some((else_value, else_ty)) = self.lower_expr(otherwise) else {
            self.close_region();
            return None;
        };
        if !then_ty.compatible(&else_ty) {
            self.close_region();
            self.session.report(type_error(
                otherwise.span,
                format!(
                    "conditional arms differ: {} and {}",
                    then_ty.display(),
                    else_ty.display()
                ),
            ));
            return None;
        }
        let else_value = self.ternary_arm_value(else_value, &then_ty, counted);
        self.close_region();
        self.fb.branch(join.clone(), vec![else_value]);

        self.fb.switch_to(&join);
        if counted {
            return Some(self.fresh(result_param, then_ty));
        }
        Some((result_param, then_ty))
    }

    /// An arm's value as it leaves the arm: a copy of its own when the
    /// conditional's value holds buffers.
    fn ternary_arm_value(&mut self, value: Name, ty: &FeType, counted: bool) -> Name {
        if !counted {
            return value;
        }
        let storage = self.fb.stack_alloc(ty.to_kir());
        self.store_value(klf_kir::StoreMode::Init, value, ty, storage.clone());
        storage
    }
}

fn comparison(op: BinOp, signed: bool) -> Option<IntegerPredicate> {
    let predicate = match op {
        BinOp::Eq => IntegerPredicate::Eq,
        BinOp::Ne => IntegerPredicate::Ne,
        BinOp::Lt => {
            if signed {
                IntegerPredicate::SignedLt
            } else {
                IntegerPredicate::UnsignedLt
            }
        }
        BinOp::Le => {
            if signed {
                IntegerPredicate::SignedLe
            } else {
                IntegerPredicate::UnsignedLe
            }
        }
        BinOp::Gt => {
            if signed {
                IntegerPredicate::SignedGt
            } else {
                IntegerPredicate::UnsignedGt
            }
        }
        BinOp::Ge => {
            if signed {
                IntegerPredicate::SignedGe
            } else {
                IntegerPredicate::UnsignedGe
            }
        }
        _ => return None,
    };
    Some(predicate)
}

pub(super) enum Receiver<'a> {
    Expr(&'a Expr),
    Path(klf_kira_syntax::ast::Path),
}

/// If `callee` is a method call, return the expression preceding its method.
pub(super) fn method_receiver<'a>(callee: &'a Expr, method: &str) -> Option<Receiver<'a>> {
    match &callee.kind {
        ExprKind::Field { base, name } if name.name.as_str() == method => {
            Some(Receiver::Expr(base))
        }
        ExprKind::Path(path)
            if path.segments.len() >= 2
                && path
                    .segments
                    .last()
                    .is_some_and(|s| s.name.as_str() == method) =>
        {
            let segments = path.segments[..path.segments.len() - 1].to_vec();
            Some(Receiver::Path(klf_kira_syntax::ast::Path {
                segments,
                span: path.span,
            }))
        }
        _ => None,
    }
}

/// If `callee` is `receiver.name`, return the receiver and the method name.
pub(super) fn method_call(callee: &Expr) -> Option<(Receiver<'_>, Ident)> {
    let method = match &callee.kind {
        ExprKind::Field { name, .. } => name.clone(),
        ExprKind::Path(path) if path.segments.len() >= 2 => path.segments.last()?.clone(),
        _ => return None,
    };
    let receiver = method_receiver(callee, method.name.as_str())?;
    Some((receiver, method))
}

/// If `callee` is `receiver.append`, return the receiver as a place to lower.
pub(super) fn append_receiver(callee: &Expr) -> Option<Receiver<'_>> {
    method_receiver(callee, "append")
}

pub(super) fn is_string(ty: &FeType) -> bool {
    matches!(ty, FeType::Struct(name) if name.as_str() == "String")
}

/// The header struct name for a value that supports field access: a nominal
/// struct, or the `List` header behind a growable array.
fn float_comparison(op: BinOp) -> Option<FloatPredicate> {
    let predicate = match op {
        BinOp::Eq => FloatPredicate::Eq,
        BinOp::Ne => FloatPredicate::Ne,
        BinOp::Lt => FloatPredicate::Lt,
        BinOp::Le => FloatPredicate::Le,
        BinOp::Gt => FloatPredicate::Gt,
        BinOp::Ge => FloatPredicate::Ge,
        _ => return None,
    };
    Some(predicate)
}

fn parse_int(text: &str) -> Option<i128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, rest)
    } else if let Some(rest) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, rest)
    } else {
        (10, cleaned.as_str())
    };
    i128::from_str_radix(digits, radix).ok()
}
