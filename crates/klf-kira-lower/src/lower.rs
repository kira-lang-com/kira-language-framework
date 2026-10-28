//! Statement lowering and the `Lowerer` shared with expression lowering.

mod aggregates;
mod call;
mod cli;
pub(crate) mod closures;
pub(crate) mod compat_driver;
pub(crate) mod project_hook;
mod control;
mod definition;
mod driver_flags;
mod editor;
mod entry;
pub(crate) mod expr;
mod file_modes;
mod files;
pub(crate) mod format_hook;
mod host_text;
mod lists;
mod literals;
mod match_arms;
pub(crate) mod ownership;
mod place;
mod process;
pub(crate) mod run_program;
mod services;
pub(crate) mod shader_emit;
mod strings;
mod testing;
mod text;
mod text_match;
mod trap_report;
mod try_expr;

pub(crate) use entry::lower_function;

use crate::ty::FeType;
use crate::{Signature, type_error};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::StoreMode;
use klf_kira_syntax::ast::{Block, Expr, ExprKind, Stmt, StmtKind};
use klf_services::{FunctionBuilder, Session, Solution};
use klf_source::Span;
use std::collections::HashMap;

/// The result of offering a call to a builtin-lowering family.
///
/// The families are tried in order, so "this is not mine" has to stay distinct
/// from "this is mine and it failed"; otherwise a reported error would fall
/// through to the next family and be reported again as an unknown function.
pub(crate) enum Builtin {
    /// Not a member of this family. Keep looking.
    Skipped,
    /// A member of this family whose lowering already reported a diagnostic.
    Failed,
    Lowered(Name, FeType),
}

impl Builtin {
    /// Wraps a family entry point that returns `None` after reporting.
    pub(crate) fn from_lowered(result: Option<(Name, FeType)>) -> Self {
        match result {
            Some((value, ty)) => Self::Lowered(value, ty),
            None => Self::Failed,
        }
    }

    /// The value for a caller that has run out of families to try.
    pub(crate) fn into_value(self) -> Option<(Name, FeType)> {
        match self {
            Self::Lowered(value, ty) => Some((value, ty)),
            Self::Skipped | Self::Failed => None,
        }
    }
}

/// A generic function lowered for one choice of type arguments: the body
/// of `base` with each type parameter read as the type it stands for, so
/// every method, copy and count in it is that type's own.
#[derive(Clone)]
pub(crate) struct Instance {
    pub name: Name,
    pub base: Name,
    pub substitutions: HashMap<Name, FeType>,
}

impl Instance {
    /// The instance of `base` for the type arguments of `generics`, named
    /// the way the generics engine names a specialization (section 13).
    pub(crate) fn of(
        base: &Name,
        generics: &[Name],
        substitutions: &HashMap<Name, FeType>,
    ) -> Self {
        let arguments: Vec<String> = generics
            .iter()
            .map(|generic| {
                substitutions
                    .get(generic)
                    .map_or_else(|| generic.to_string(), FeType::spelling)
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect()
            })
            .collect();
        Self {
            name: Name::new(format!("{base}${}", arguments.join("."))),
            base: base.clone(),
            substitutions: substitutions.clone(),
        }
    }
}

pub(crate) struct Local {
    pub access: Access,
    pub ty: FeType,
    pub mutable: bool,
    /// Storage the function reads but neither writes nor owns, when it is.
    pub shared: Option<Shared>,
}

/// Why a local reads storage it does not own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shared {
    /// A parameter passed without `move`, `copy` or `mut`: the caller keeps
    /// the value, and nothing in the body may write it.
    Parameter,
    /// A `let` of a place inside such a parameter, bound to that place.
    Alias,
}

pub(crate) enum Access {
    Ssa(Name),
    Slot(Name),
}

struct LoopCtx {
    continue_block: Name,
    break_block: Name,
    /// The temporary regions and scopes open where the body starts, which a
    /// jump out of the body leaves.
    regions: usize,
    scopes: usize,
}

pub(crate) struct Lowerer<'a> {
    pub session: &'a mut Session,
    elaborated: &'a crate::ElaboratedFile,
    pub signatures: &'a HashMap<Name, Signature>,
    pub structs: &'a crate::structs::StructTable,
    pub enums: &'a crate::enums::EnumTable,
    pub types: &'a crate::session_types::TypeHandles,
    pub constants: &'a crate::constants::ConstantTable,
    pub generic_names: std::collections::HashSet<Name>,
    /// In an instance of a generic function, the type each parameter is.
    pub substitutions: HashMap<Name, FeType>,
    pub fb: FunctionBuilder,
    scopes: Vec<Vec<(Name, Local)>>,
    /// Per scope, the storage it owns.
    owned: Vec<Vec<ownership::Owned>>,
    /// The temporaries of each open statement, condition and operand.
    regions: Vec<Vec<ownership::Owned>>,
    counts: &'a std::cell::RefCell<ownership::NeededFunctions>,
    result_ty: FeType,
    ret_slot: Option<Name>,
    reachable: bool,
    loops: Vec<LoopCtx>,
    /// Set in the `units` phase: the session its `[Unit]` result is pushed to.
    definition_session: Option<Name>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(
        session: &'a mut Session,
        elaborated: &'a crate::ElaboratedFile,
        counts: &'a std::cell::RefCell<ownership::NeededFunctions>,
        fb: FunctionBuilder,
        result_ty: FeType,
        ret_slot: Option<Name>,
    ) -> Self {
        Self {
            session,
            elaborated,
            signatures: &elaborated.signatures,
            structs: &elaborated.structs,
            enums: &elaborated.enums,
            types: &elaborated.types,
            constants: &elaborated.constants,
            generic_names: std::collections::HashSet::new(),
            substitutions: HashMap::new(),
            fb,
            scopes: Vec::new(),
            owned: Vec::new(),
            regions: Vec::new(),
            counts,
            result_ty,
            ret_slot,
            reachable: true,
            loops: Vec::new(),
            definition_session: None,
        }
    }
}

impl Lowerer<'_> {
    /// Records where the source being lowered came from, so every instruction
    /// built from it lands on the right line of the line table (section 16.3).
    pub(crate) fn at(&mut self, span: Span) {
        let file = self.session.file(span.file);
        let place = file.line_column(span.start);
        self.fb.at(Some(klf_kir::DebugLocation {
            file: file.name().as_str().to_owned(),
            line: place.line,
            column: place.column,
        }));
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.owned.push(Vec::new());
    }

    /// Ends the innermost scope, releasing what it owns when control reaches
    /// its end.
    fn pop_scope(&mut self) {
        self.scopes.pop();
        let owned = self.owned.pop().unwrap_or_default();
        if self.reachable {
            self.release_all_of(&owned);
        }
    }

    fn push_loop(&mut self, continue_block: Name, break_block: Name) {
        self.loops.push(LoopCtx {
            continue_block,
            break_block,
            regions: self.regions.len(),
            scopes: self.owned.len(),
        });
    }

    fn bind(&mut self, name: Name, local: Local) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .push((name, local));
    }

    pub(crate) fn lookup(&self, name: &Name) -> Option<&Local> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.iter().rev().find(|(n, _)| n == name).map(|(_, l)| l))
    }

    fn lower_block(&mut self, block: &Block) {
        self.push_scope();
        self.lower_stmts(&block.stmts);
        self.pop_scope();
    }

    fn lower_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if !self.reachable {
                break;
            }
            self.lower_stmt(stmt);
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt) {
        self.at(stmt.span);
        self.open_region();
        self.lower_stmt_kind(stmt);
        self.close_region();
    }

    fn lower_stmt_kind(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let {
                is_var,
                name,
                ty,
                value,
            } => self.lower_let(*is_var, name, ty.as_ref(), value),
            StmtKind::Assign { target, op, value } => self.lower_assign(target, *op, value),
            StmtKind::Expr(expr) => {
                self.lower_expr(expr);
            }
            StmtKind::If {
                branches,
                else_block,
            } => self.lower_if(branches, else_block.as_ref()),
            StmtKind::While { cond, body } => self.lower_while(cond, body),
            StmtKind::Return(value) => self.lower_return(value.as_ref(), stmt.span),
            StmtKind::Break => self.lower_jump(true, stmt.span),
            StmtKind::Continue => self.lower_jump(false, stmt.span),
            StmtKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms, stmt.span),
            StmtKind::For { var, iter, body } => self.lower_for(var, iter, body, stmt.span),
            StmtKind::Attempt { .. } => {
                self.session.report(
                    Diagnostic::error("KLFK-E0206", "this statement is outside the M4 subset")
                        .with_label(stmt.span, "not lowered yet"),
                );
            }
        }
    }

    pub(crate) fn resolve(&mut self, ty: &klf_kira_syntax::ast::Type) -> Option<FeType> {
        let struct_names: std::collections::HashSet<Name> = self.structs.keys().cloned().collect();
        let enum_names: std::collections::HashSet<Name> = self.enums.keys().cloned().collect();
        crate::structs::resolve_type_with_context(
            self.session,
            ty,
            &struct_names,
            &enum_names,
            &self.generic_names,
        )
        .map(|resolved| resolved.substitute(&self.substitutions))
    }

    pub(crate) fn check_argument_type(
        &mut self,
        expected: &FeType,
        actual: &FeType,
        origin: Span,
    ) -> bool {
        let expected_id = self.types.id_for(self.session, expected);
        let actual_id = self.types.id_for(self.session, actual);
        let solution = {
            let mut solver = self.session.solver();
            solver.equate(expected_id, actual_id, origin);
            solver.solve(1)
        };
        if matches!(solution, Solution::Unique(_)) {
            true
        } else {
            self.session.report(type_error(
                origin,
                format!(
                    "argument has type {}, expected {}",
                    actual.display(),
                    expected.display()
                ),
            ));
            false
        }
    }

    pub(crate) fn check_generic_bounds(
        &mut self,
        signature: &Signature,
        substitutions: &HashMap<Name, FeType>,
        origin: Span,
    ) -> bool {
        let mut valid = true;
        for generic in &signature.generics {
            let Some(actual) = substitutions.get(generic) else {
                continue;
            };
            let actual_id = self.types.id_for(self.session, actual);
            for trait_id in signature.bounds.get(generic).into_iter().flatten() {
                let solution = {
                    let mut solver = self.session.solver();
                    solver.conform(actual_id, *trait_id, origin);
                    solver.solve(1)
                };
                if matches!(solution, Solution::Unique(_)) {
                    continue;
                }
                let trait_name = self
                    .session
                    .trait_info(*trait_id)
                    .map_or_else(|| "<unknown>".to_owned(), |info| info.name.to_string());
                self.session.report(
                    Diagnostic::error(
                        "KLFK-E0202",
                        format!(
                            "inferred type {} does not conform to `{trait_name}`",
                            actual.display()
                        ),
                    )
                    .with_label(origin, "required by the generic bound"),
                );
                valid = false;
            }
        }
        valid
    }

    fn lower_let(
        &mut self,
        is_var: bool,
        name: &klf_kira_syntax::ast::Ident,
        annotation: Option<&klf_kira_syntax::ast::Type>,
        expr: &Expr,
    ) {
        let expected = annotation.and_then(|ann| self.resolve(ann));
        let Some((mut v, vty)) = self.lower_expr_expected(expr, expected.as_ref()) else {
            return;
        };
        // An aggregate's value is the address of its storage, so a binding
        // owns storage of its own: it takes over a temporary the statement
        // built, and copies anything else. A `let` of a value without buffers
        // read from somewhere other than a variable, field or element shares
        // its storage, which nothing else writes.
        let shared = (!is_var && self.shared_root(expr)).then_some(Shared::Alias);
        if shared.is_some() {
            // A `let` of a place inside a parameter the body cannot write
            // reads that place for as long as it lives, and copies nothing.
        } else if self.counted(&vty) {
            v = self.take_owned(v, &vty);
            self.own_in_scope(v.clone(), vty.clone());
        } else if lists::is_aggregate(&vty)
            && (is_var
                || matches!(
                    expr.kind,
                    ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
                ))
        {
            let copy = self.fb.stack_alloc(vty.to_kir());
            self.store_value(StoreMode::Init, v, &vty, copy.clone());
            v = copy;
        }
        if let Some(at) = &expected
            && !at.compatible(&vty)
        {
            self.session.report(type_error(
                annotation.map_or(name.span, |ann| ann.span),
                format!(
                    "binding annotated {} but initialized with {}",
                    at.display(),
                    vty.display()
                ),
            ));
        }
        // A `var` gets a mutable stack slot so it can be reassigned: a scalar
        // slot holds the value, an aggregate slot holds the storage address. A
        // `let` binds the value (or aggregate address) directly.
        let access = if is_var {
            let slot_ty = if vty.is_scalar() {
                vty.to_kir()
            } else {
                vty.value_kir()
            };
            let slot = self.fb.stack_alloc(slot_ty);
            self.fb.store(StoreMode::Init, v, slot.clone());
            Access::Slot(slot)
        } else {
            Access::Ssa(v)
        };
        self.bind(
            name.name.clone(),
            Local {
                access,
                ty: vty,
                mutable: is_var,
                shared,
            },
        );
    }

    /// Whether `expr` names a place inside a parameter the body cannot
    /// write, through fields and elements.
    pub(crate) fn shared_root(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Path(path) if path.segments.len() == 1 => self
                .lookup(&path.segments[0].name)
                .is_some_and(|local| local.shared.is_some() && !local.mutable),
            ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => self.shared_root(base),
            _ => false,
        }
    }

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

    fn lower_assign(&mut self, target: &Expr, op: klf_kira_syntax::ast::AssignOp, value: &Expr) {
        if let ExprKind::Field { base, name } = &target.kind {
            self.lower_field_assign(base, name, op, value, target.span);
            return;
        }
        if let ExprKind::Index { base, index } = &target.kind {
            let Some((address, element_ty)) = self.index_place_for(base, index, target.span, true) else {
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
    fn lower_condition(&mut self, cond: &Expr) -> Option<Name> {
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

    fn lower_return(&mut self, value: Option<&Expr>, span: klf_source::Span) {
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

    fn lower_jump(&mut self, is_break: bool, span: klf_source::Span) {
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
