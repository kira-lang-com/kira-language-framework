//! Lowering the calculator AST to KIR through the kernel builder.

use crate::syntax::{BinOp, CmpOp, Def, Expr};
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{BinaryOperator, IntegerPredicate, Module, Overflow, Type};
use klf_services::{FunctionBuilder, ModuleBuilder, Session};
use klf_source::Span;
use klf_target::Target;
use std::collections::{HashMap, HashSet};

const I64: fn() -> Type = || Type::Integer(64);

pub(crate) fn lower_with_arities(
    session: &mut Session,
    module_name: &str,
    target: Target,
    defs: &[Def],
    arities: &HashMap<String, usize>,
) -> Option<Module> {
    let mut module = ModuleBuilder::new(Name::new(module_name), "calc", target);
    for def in defs {
        let function = lower_def(session, arities, def);
        module.add_function(function);
    }
    if session.error_count() == 0 {
        Some(module.finish())
    } else {
        None
    }
}

fn lower_def(
    session: &mut Session,
    arities: &HashMap<String, usize>,
    def: &Def,
) -> klf_kir::Function {
    let params: Vec<(Name, Type)> = def.params.iter().map(|p| (Name::new(p), I64())).collect();
    let fb = FunctionBuilder::new(Name::new(&def.name), &params, I64(), def.span);
    let mut lowerer = Lowerer {
        session,
        arities,
        fb,
        params: def.params.iter().cloned().collect(),
    };
    if let Some(value) = lowerer.lower_expr(&def.body) {
        lowerer.fb.ret(Some(value));
    } else {
        lowerer.fb.trap("calc lowering error");
    }
    lowerer.fb.finish()
}

struct Lowerer<'a> {
    session: &'a mut Session,
    arities: &'a HashMap<String, usize>,
    fb: FunctionBuilder,
    params: HashSet<String>,
}

impl Lowerer<'_> {
    fn lower_expr(&mut self, expr: &Expr) -> Option<Name> {
        match expr {
            Expr::Int(value) => Some(self.fb.const_int(I64(), i128::from(*value))),
            Expr::Var(name) => {
                if self.params.contains(name) {
                    Some(Name::new(name))
                } else {
                    self.session.report(Diagnostic::error(
                        "CALC-E0002",
                        format!("unknown variable `{name}`"),
                    ));
                    None
                }
            }
            Expr::Neg(inner) => {
                let value = self.lower_expr(inner)?;
                let zero = self.fb.const_int(I64(), 0);
                Some(
                    self.fb
                        .binary(BinaryOperator::Sub, Overflow::Trap, true, zero, value),
                )
            }
            Expr::Bin(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let (operator, overflow) = match op {
                    BinOp::Add => (BinaryOperator::Add, Overflow::Trap),
                    BinOp::Sub => (BinaryOperator::Sub, Overflow::Trap),
                    BinOp::Mul => (BinaryOperator::Mul, Overflow::Trap),
                    BinOp::Div => (BinaryOperator::SignedDiv, Overflow::Wrap),
                    BinOp::Rem => (BinaryOperator::SignedRem, Overflow::Wrap),
                };
                Some(self.fb.binary(operator, overflow, true, l, r))
            }
            Expr::Call(name, args, span) => self.lower_call(name, args, *span),
            Expr::If {
                lhs,
                op,
                rhs,
                then,
                els,
            } => self.lower_if(lhs, *op, rhs, then, els),
        }
    }

    fn lower_call(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Name> {
        let Some(&arity) = self.arities.get(name) else {
            self.session.report(
                Diagnostic::error("CALC-E0002", format!("unknown function `{name}`"))
                    .with_label(span, "not defined"),
            );
            return None;
        };
        if args.len() != arity {
            self.session.report(
                Diagnostic::error(
                    "CALC-E0003",
                    format!("`{name}` takes {arity} arguments, got {}", args.len()),
                )
                .with_label(span, "wrong argument count"),
            );
            return None;
        }
        let mut values = Vec::new();
        for arg in args {
            values.push(self.lower_expr(arg)?);
        }
        Some(self.fb.call(Name::new(name), values))
    }

    fn lower_if(
        &mut self,
        lhs: &Expr,
        op: CmpOp,
        rhs: &Expr,
        then: &Expr,
        els: &Expr,
    ) -> Option<Name> {
        let l = self.lower_expr(lhs)?;
        let r = self.lower_expr(rhs)?;
        let predicate = match op {
            CmpOp::Eq => IntegerPredicate::Eq,
            CmpOp::Ne => IntegerPredicate::Ne,
            CmpOp::Lt => IntegerPredicate::SignedLt,
            CmpOp::Le => IntegerPredicate::SignedLe,
            CmpOp::Gt => IntegerPredicate::SignedGt,
            CmpOp::Ge => IntegerPredicate::SignedGe,
        };
        let cond = self.fb.compare(predicate, l, r);
        let then_block = self.fb.new_block(&[]);
        let else_block = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            cond,
            then_block.clone(),
            Vec::new(),
            else_block.clone(),
            Vec::new(),
        );

        self.fb.switch_to(&then_block);
        let then_value = self.lower_expr(then)?;
        let result = self.fb.fresh_value();
        let join = self.fb.new_block(&[(result.clone(), I64())]);
        self.fb.branch(join.clone(), vec![then_value]);

        self.fb.switch_to(&else_block);
        let else_value = self.lower_expr(els)?;
        self.fb.branch(join.clone(), vec![else_value]);

        self.fb.switch_to(&join);
        Some(result)
    }
}
