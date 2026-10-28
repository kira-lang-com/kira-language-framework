//! `try operand`: the `Ok` payload of a Result-shaped value, or an early
//! return that hands its `Error` payload to the caller.
//!
//! A Result-shaped enum has an `Ok` and an `Error` case with one payload
//! each (section 6.2). The enclosing function must itself return a
//! Result-shaped enum whose `Error` payload has the same type, which it fills
//! through its out-pointer before returning.

use super::Lowerer;
use super::lists::is_aggregate;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{BlockTarget, StoreMode, Type};
use klf_kira_syntax::ast::Expr;
use klf_source::Span;

/// The case indices and payload types of a Result-shaped enum.
struct ResultShape {
    ok: u32,
    ok_payload: FeType,
    error: u32,
    error_payload: FeType,
}

impl Lowerer<'_> {
    pub(super) fn lower_try(&mut self, operand: &Expr, span: Span) -> Option<(Name, FeType)> {
        let (value, operand_ty) = self.lower_expr(operand)?;
        let Some(source) = self.result_shape(&operand_ty) else {
            self.session.report(type_error(
                operand.span,
                format!(
                    "`try` needs a Result-shaped value, found {}",
                    operand_ty.display()
                ),
            ));
            return None;
        };
        let result_ty = self.result_ty.clone();
        let Some(target) = self.result_shape(&result_ty) else {
            self.session.report(type_error(
                span,
                "`try` is only valid in a function that returns a Result-shaped enum",
            ));
            return None;
        };
        if !target.error_payload.compatible(&source.error_payload) {
            self.session.report(type_error(
                span,
                format!(
                    "`try` passes on a {} failure from a function whose failure is {}",
                    source.error_payload.display(),
                    target.error_payload.display()
                ),
            ));
            return None;
        }
        let destination = self.ret_slot.clone()?;

        let ok_payload = self.fb.fresh_value();
        let ok_block = self.fb.new_block(&[(
            ok_payload.clone(),
            Type::Address(Box::new(source.ok_payload.to_kir())),
        )]);
        let error_payload = self.fb.fresh_value();
        let error_block = self.fb.new_block(&[(
            error_payload.clone(),
            Type::Address(Box::new(source.error_payload.to_kir())),
        )]);
        let ok_address = self.fb.enum_payload_addr(value.clone(), source.ok);
        let error_address = self.fb.enum_payload_addr(value.clone(), source.error);
        self.fb.switch_enum(
            value,
            vec![
                (
                    source.ok,
                    BlockTarget {
                        block: ok_block.clone(),
                        arguments: vec![ok_address],
                    },
                ),
                (
                    source.error,
                    BlockTarget {
                        block: error_block.clone(),
                        arguments: vec![error_address],
                    },
                ),
            ],
            None,
        );

        self.fb.switch_to(&error_block);
        let failure = if is_aggregate(&source.error_payload) {
            error_payload
        } else {
            self.fb.load(error_payload)
        };
        let place = self.fb.enum_payload_addr(destination.clone(), target.error);
        self.store_value(StoreMode::Init, failure, &target.error_payload, place);
        self.fb.enum_inject(destination, target.error);
        self.release_all();
        self.fb.ret(None);

        self.fb.switch_to(&ok_block);
        Some(self.load_place(ok_payload, source.ok_payload))
    }

    fn result_shape(&self, ty: &FeType) -> Option<ResultShape> {
        let FeType::Enum(name) = ty else {
            return None;
        };
        let info = self.enums.get(name)?;
        if info.variants.len() != 2 {
            return None;
        }
        let (ok, ok_case) = info.variant("Ok")?;
        let (error, error_case) = info.variant("Error")?;
        let [ok_payload] = ok_case.payload.as_slice() else {
            return None;
        };
        let [error_payload] = error_case.payload.as_slice() else {
            return None;
        };
        Some(ResultShape {
            ok,
            ok_payload: ok_payload.clone(),
            error,
            error_payload: error_payload.clone(),
        })
    }
}
