//! Closures (section 6.2): a literal `{ a, b in ... }` and the calls made
//! through a value of function type.
//!
//! A closure value is a record of two words (section 21.8): `code`, the
//! function its body lowers to, and `ctx`, a counted box holding a copy of
//! every local the body reads from the scope it was written in. The box
//! starts with the function that releases those copies, so the last holder
//! of a closure releases its captures without knowing what they are. `code`
//! takes the box in the context register, then the closure's parameters; an
//! aggregate result goes through an out-pointer before them, as a declared
//! function's does.

use super::{Access, Local, Lowerer, Shared};
use crate::structs::StructInfo;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{ConvertKind, Linkage, StoreMode, Type};
use klf_kira_syntax::ast::{Arg, Block, Expr, ExprKind, Ident, Stmt, StmtKind};
use klf_services::FunctionBuilder;
use klf_source::Span;

/// The context register parameter of a closure's code.
const CONTEXT: &str = "__context";

impl Lowerer<'_> {
    /// `{ a, b in body }` where a value of function type is expected.
    pub(super) fn lower_closure(
        &mut self,
        params: &[Ident],
        body: &[Stmt],
        expected: Option<&FeType>,
        span: Span,
    ) -> Option<(Name, FeType)> {
        let Some(FeType::Function {
            params: types,
            result,
        }) = expected.cloned()
        else {
            self.session.report(type_error(
                span,
                "a closure needs a function type here, from a parameter or an annotation",
            ));
            return None;
        };
        if params.len() != types.len() {
            self.session.report(type_error(
                span,
                format!(
                    "the closure takes {} parameters, its type {}",
                    params.len(),
                    types.len()
                ),
            ));
            return None;
        }
        if types.iter().any(FeType::is_open) {
            self.session.report(type_error(
                span,
                "the closure's parameter types are not known here; annotate the binding",
            ));
            return None;
        }
        let captures = self.captures(params, body, span);
        let result = if result.is_open() {
            Box::new(self.closure_result(params, &types, body, &captures, span)?)
        } else {
            result
        };
        let ty = FeType::Function {
            params: types.clone(),
            result: result.clone(),
        };
        let (code, context) = self.closure_names();
        let fields = std::iter::once((Name::new("release"), FeType::Pointer))
            .chain(
                captures
                    .iter()
                    .map(|(name, _, ty)| (name.clone(), ty.clone())),
            )
            .collect();
        self.counts.borrow_mut().contexts.insert(
            context.clone(),
            StructInfo {
                span,
                fields,
                defaults: Vec::new(),
                wrapped: Vec::new(),
            },
        );
        let function = self.closure_code(
            &code, &context, params, &types, &result, body, &captures, span,
        );
        self.counts.borrow_mut().closures.push(function);

        let context_ty = Type::Nominal(context.clone(), Vec::new());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let buffer = self.fb.heap_alloc_array(context_ty.clone(), one);
        let storage = self.fb.convert(
            ConvertKind::Bitcast,
            buffer.clone(),
            Type::Address(Box::new(context_ty)),
        );
        let release =
            self.count_function(super::ownership::Count::Release, &FeType::Struct(context));
        let release = self.fb.function_reference(release);
        let release = self
            .fb
            .convert(ConvertKind::Bitcast, release, Type::Pointer);
        let slot = self.fb.field_addr(storage.clone(), 0);
        self.fb.store(StoreMode::Init, release, slot);
        for (index, (_, value, ty)) in captures.into_iter().enumerate() {
            let field = u32::try_from(index + 1).unwrap_or(0);
            let slot = self.fb.field_addr(storage.clone(), field);
            self.store_value(StoreMode::Init, value, &ty, slot);
        }

        let record = self.fb.stack_alloc(ty.to_kir());
        let code = self.fb.function_reference(code);
        let code = self.fb.convert(ConvertKind::Bitcast, code, Type::Pointer);
        let slot = self.fb.field_addr(record.clone(), 0);
        self.fb.store(StoreMode::Init, code, slot);
        let context = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        let slot = self.fb.field_addr(record.clone(), 1);
        self.fb.store(StoreMode::Init, context, slot);
        Some(self.fresh(record, ty))
    }

    /// The names of the next closure's code and of its context's struct.
    fn closure_names(&self) -> (Name, Name) {
        let index = self.counts.borrow().closures.len() + self.counts.borrow().contexts.len();
        (
            Name::new(format!("__closure{index}")),
            Name::new(format!("__closure{index}.context")),
        )
    }

    /// Each local of the enclosing scopes the body names, with its value now
    /// and its type. `self` is captured whenever the body names anything
    /// that is not a local, since that may be one of its members.
    fn captures(
        &mut self,
        params: &[Ident],
        body: &[Stmt],
        span: Span,
    ) -> Vec<(Name, Name, FeType)> {
        let mut named = Vec::new();
        stmts_names(body, &mut named);
        let receiver = Name::new("self");
        if named.iter().any(|name| self.lookup(name).is_none()) {
            named.push(receiver);
        }
        let mut captures: Vec<(Name, Name, FeType)> = Vec::new();
        for name in named {
            if params.iter().any(|param| param.name == name)
                || captures.iter().any(|(captured, _, _)| *captured == name)
                || self.lookup(&name).is_none()
            {
                continue;
            }
            let path = klf_kira_syntax::ast::Path {
                segments: vec![Ident {
                    name: name.clone(),
                    span,
                }],
                span,
            };
            if let Some((value, ty)) = self.lower_path(&path, span) {
                captures.push((name, value, ty));
            }
        }
        captures
    }

    /// The function a closure's body lowers to.
    #[allow(clippy::too_many_arguments)]
    fn closure_code(
        &mut self,
        code: &Name,
        context: &Name,
        params: &[Ident],
        types: &[FeType],
        result: &FeType,
        body: &[Stmt],
        captures: &[(Name, Name, FeType)],
        span: Span,
    ) -> klf_kir::Function {
        let aggregate_result = super::lists::is_aggregate(result);
        let mut kir = vec![(Name::new(CONTEXT), Type::Pointer)];
        let ret_slot = aggregate_result.then(|| Name::new("__ret"));
        if let Some(slot) = &ret_slot {
            kir.push((slot.clone(), result.value_kir()));
        }
        for (param, ty) in params.iter().zip(types) {
            kir.push((param.name.clone(), ty.value_kir()));
        }
        let kir_result = if aggregate_result {
            Type::Unit
        } else {
            result.to_kir()
        };
        let mut fb = FunctionBuilder::new(code.clone(), &kir, kir_result, span);
        fb.set_linkage(Linkage::Module);
        let mut lowerer = Lowerer::new(
            &mut *self.session,
            self.elaborated,
            self.counts,
            fb,
            result.clone(),
            ret_slot,
        );
        lowerer.generic_names.clone_from(&self.generic_names);
        lowerer.substitutions.clone_from(&self.substitutions);
        lowerer.push_scope();
        let context_ty = Type::Address(Box::new(Type::Nominal(context.clone(), Vec::new())));
        let storage = lowerer
            .fb
            .convert(ConvertKind::Bitcast, Name::new(CONTEXT), context_ty);
        for (index, (name, _, ty)) in captures.iter().enumerate() {
            let field = u32::try_from(index + 1).unwrap_or(0);
            let place = lowerer.fb.field_addr(storage.clone(), field);
            let value = if super::lists::is_aggregate(ty) {
                place
            } else {
                lowerer.fb.load(place)
            };
            lowerer.bind_read_only(name.clone(), value, ty.clone());
        }
        for (param, ty) in params.iter().zip(types) {
            lowerer.bind_read_only(param.name.clone(), param.name.clone(), ty.clone());
        }
        match body.split_last() {
            Some((
                Stmt {
                    kind: StmtKind::Expr(last),
                    span: last_span,
                },
                rest,
            )) if *result != FeType::Void => {
                lowerer.lower_stmts(rest);
                if lowerer.reachable {
                    lowerer.at(*last_span);
                    lowerer.open_region();
                    lowerer.lower_return(Some(last), *last_span);
                    lowerer.close_region();
                }
            }
            _ => lowerer.lower_stmts(body),
        }
        lowerer.pop_scope();
        super::entry::close_body(&mut lowerer, result, span);
        let mut function = lowerer.fb.finish();
        function.abi = klf_kir::FunctionAbi::Ridl;
        if let Some(parameter) = function.parameters.first_mut() {
            parameter.context = true;
        }
        function
    }

    /// The type a closure's body gives its last expression, found by
    /// lowering the body once and throwing that lowering away.
    fn closure_result(
        &mut self,
        params: &[Ident],
        types: &[FeType],
        body: &[Stmt],
        captures: &[(Name, Name, FeType)],
        span: Span,
    ) -> Option<FeType> {
        let Some((
            Stmt {
                kind: StmtKind::Expr(last),
                ..
            },
            rest,
        )) = body.split_last()
        else {
            return Some(FeType::Void);
        };
        let elaborated = self.elaborated;
        let counts = self.counts;
        let generic_names = self.generic_names.clone();
        let substitutions = self.substitutions.clone();
        let (closures, contexts) = {
            let counts = counts.borrow();
            (
                counts.closures.len(),
                counts.contexts.keys().cloned().collect::<Vec<_>>(),
            )
        };
        let mut found = None;
        let errors = self.session.error_count();
        let _: Option<()> = self.session.speculate(|session| {
            let fb = FunctionBuilder::new(Name::new("__probe"), &[], Type::Unit, span);
            let mut lowerer = Lowerer::new(session, elaborated, counts, fb, FeType::Void, None);
            lowerer.generic_names = generic_names;
            lowerer.substitutions = substitutions;
            lowerer.push_scope();
            for (name, value, ty) in captures {
                lowerer.bind_read_only(name.clone(), value.clone(), ty.clone());
            }
            for (param, ty) in params.iter().zip(types) {
                lowerer.bind_read_only(param.name.clone(), param.name.clone(), ty.clone());
            }
            lowerer.lower_stmts(rest);
            if lowerer.reachable && lowerer.session.error_count() == errors {
                found = lowerer.lower_expr(last).map(|(_, ty)| ty);
            }
            None
        });
        let mut counts = counts.borrow_mut();
        counts.closures.truncate(closures);
        counts.contexts.retain(|name, _| contexts.contains(name));
        if found.is_none() {
            self.session.report(type_error(
                span,
                "the closure's result type cannot be found here; annotate the binding",
            ));
        }
        found
    }

    /// Binds a value the body reads and neither writes nor releases.
    fn bind_read_only(&mut self, name: Name, value: Name, ty: FeType) {
        self.bind(
            name,
            Local {
                access: Access::Ssa(value),
                ty,
                mutable: false,
                shared: Some(Shared::Parameter),
            },
        );
    }

    /// A call through the closure value `closure`.
    pub(super) fn lower_closure_call(
        &mut self,
        closure: (Name, FeType),
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let FeType::Function { params, result } = closure.1 else {
            return None;
        };
        if args.len() != params.len() {
            self.session.report(type_error(
                span,
                format!(
                    "the closure takes {} arguments, got {}",
                    params.len(),
                    args.len()
                ),
            ));
            return None;
        }
        let mut values = Vec::with_capacity(args.len() + 1);
        let mut kir = Vec::with_capacity(args.len() + 1);
        let destination = super::lists::is_aggregate(&result).then(|| {
            let destination = self.fb.stack_alloc(result.to_kir());
            values.push(destination.clone());
            kir.push(result.value_kir());
            destination
        });
        for (arg, expected) in args.iter().zip(&params) {
            let (value, ty) = self.lower_expr_expected(&arg.value, Some(expected))?;
            if !self.check_argument_type(expected, &ty, arg.value.span) {
                return None;
            }
            values.push(self.detach(&arg.value, value, &ty));
            kir.push(expected.value_kir());
        }
        if let Some(destination) = destination {
            self.fb.call_closure_void(closure.0, kir, values);
            return Some(self.fresh(destination, *result));
        }
        if *result == FeType::Void {
            self.fb.call_closure_void(closure.0, kir, values);
            return Some((self.fb.const_int(Type::Integer(64), 0), FeType::Void));
        }
        let value = self
            .fb
            .call_closure(closure.0, kir, result.to_kir(), values);
        Some((value, *result))
    }
}

/// The context structs closures declared, as KIR types.
pub(crate) fn context_declarations(
    contexts: &crate::structs::StructTable,
) -> Vec<klf_kir::TypeDeclaration> {
    crate::structs::type_declarations(contexts)
}

/// Every name a closure body reads as the first segment of a path.
fn stmts_names(stmts: &[Stmt], names: &mut Vec<Name>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { value, .. } => expr_names(value, names),
            StmtKind::Assign { target, value, .. } => {
                expr_names(target, names);
                expr_names(value, names);
            }
            StmtKind::Expr(expr) => expr_names(expr, names),
            StmtKind::If {
                branches,
                else_block,
            } => {
                for (condition, body) in branches {
                    expr_names(condition, names);
                    block_names(body, names);
                }
                if let Some(body) = else_block {
                    block_names(body, names);
                }
            }
            StmtKind::While { cond, body } => {
                expr_names(cond, names);
                block_names(body, names);
            }
            StmtKind::For { iter, body, .. } => {
                expr_names(iter, names);
                block_names(body, names);
            }
            StmtKind::Match { scrutinee, arms } => {
                expr_names(scrutinee, names);
                for arm in arms {
                    block_names(&arm.body, names);
                }
            }
            StmtKind::Attempt { body, handlers } => {
                block_names(body, names);
                for handler in handlers {
                    block_names(&handler.body, names);
                }
            }
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    expr_names(value, names);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn block_names(block: &Block, names: &mut Vec<Name>) {
    stmts_names(&block.stmts, names);
}

fn expr_names(expr: &Expr, names: &mut Vec<Name>) {
    match &expr.kind {
        ExprKind::Path(path) => {
            if let Some(first) = path.segments.first() {
                names.push(first.name.clone());
            }
        }
        ExprKind::Try(operand)
        | ExprKind::Unary { operand, .. }
        | ExprKind::As { expr: operand, .. }
        | ExprKind::Is { expr: operand, .. }
        | ExprKind::Field { base: operand, .. } => expr_names(operand, names),
        ExprKind::EnumLit { args, .. } => {
            for arg in args.iter().flatten() {
                expr_names(&arg.value, names);
            }
        }
        ExprKind::Tuple(items) | ExprKind::ArrayLit(items) => {
            for item in items {
                expr_names(item, names);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, value) in fields {
                expr_names(value, names);
            }
        }
        ExprKind::MapLit(entries) => {
            for (key, value) in entries {
                expr_names(key, names);
                expr_names(value, names);
            }
        }
        ExprKind::Closure { body, .. } => stmts_names(body, names),
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Range {
            start: lhs,
            end: rhs,
        }
        | ExprKind::Index {
            base: lhs,
            index: rhs,
        } => {
            expr_names(lhs, names);
            expr_names(rhs, names);
        }
        ExprKind::Ternary {
            cond,
            then,
            otherwise,
        } => {
            expr_names(cond, names);
            expr_names(then, names);
            expr_names(otherwise, names);
        }
        ExprKind::Call { callee, args } => {
            expr_names(callee, names);
            for arg in args {
                expr_names(&arg.value, names);
            }
        }
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) => {}
    }
}
