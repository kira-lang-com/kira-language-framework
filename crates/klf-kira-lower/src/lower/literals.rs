//! Struct and enum literal lowering.

use super::Lowerer;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{StoreMode, Type};
use klf_kira_syntax::ast::{Arg, Expr};
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn lower_struct_lit(
        &mut self,
        ty: &klf_kira_syntax::ast::Type,
        fields: &[(klf_kira_syntax::ast::Ident, Expr)],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let Some(FeType::Struct(name)) = self.resolve(ty) else {
            self.session.report(type_error(
                span,
                "struct literal type is not a known struct",
            ));
            return None;
        };
        let info = self.structs.get(&name)?.clone();
        let address = self
            .fb
            .stack_alloc(klf_kir::Type::Nominal(name.clone(), Vec::new()));
        let mut wrapped_values: Vec<(Name, &Expr)> = Vec::new();
        for (field_name, value_expr) in fields {
            if info.wrapped(field_name.name.as_str()).is_some() {
                wrapped_values.push((field_name.name.clone(), value_expr));
                continue;
            }
            let Some((index, field_ty)) = info.field(field_name.name.as_str()) else {
                self.session.report(type_error(
                    field_name.span,
                    format!("`{name}` has no field `{}`", field_name.name),
                ));
                continue;
            };
            let field_ty = field_ty.clone();
            let Some((value, value_ty)) = self.lower_expr_expected(value_expr, Some(&field_ty))
            else {
                continue;
            };
            if !field_ty.compatible(&value_ty) {
                self.session.report(type_error(
                    value_expr.span,
                    format!(
                        "field `{}` is {}, got {}",
                        field_name.name,
                        field_ty.display(),
                        value_ty.display()
                    ),
                ));
                continue;
            }
            let place = self.fb.field_addr(address.clone(), index);
            self.init_from(value, &field_ty, place);
        }
        // A field the literal leaves out holds the value it is declared with.
        for (field_name, default) in &info.defaults {
            if fields.iter().any(|(given, _)| given.name == *field_name) {
                continue;
            }
            let Some((index, field_ty)) = info.field(field_name.as_str()) else {
                continue;
            };
            let field_ty = field_ty.clone();
            let Some((value, _)) = self.lower_expr_expected(default, Some(&field_ty)) else {
                continue;
            };
            let place = self.fb.field_addr(address.clone(), index);
            self.init_from(value, &field_ty, place);
        }
        for wrapped in &info.wrapped {
            let given = wrapped_values
                .iter()
                .find(|(property, _)| *property == wrapped.property)
                .map(|(_, value)| *value);
            let Some(value) = given.or(wrapped.init.as_ref()) else {
                self.session.report(type_error(
                    span,
                    format!(
                        "`{name}` literal needs `{}`, which has no initial value",
                        wrapped.property
                    ),
                ));
                continue;
            };
            let value_field = klf_kira_syntax::ast::Ident {
                name: Name::new("wrappedValue"),
                span: value.span,
            };
            let mut wrapper_fields = vec![(value_field, value.clone())];
            wrapper_fields.extend(wrapped.fields.iter().cloned());
            let Some((wrapper, wrapper_ty)) =
                self.lower_struct_lit(&wrapped.wrapper, &wrapper_fields, wrapped.span)
            else {
                continue;
            };
            let Some((index, _)) = info.field(wrapped.backing.as_str()) else {
                continue;
            };
            let place = self.fb.field_addr(address.clone(), index);
            self.store_value(StoreMode::Init, wrapper, &wrapper_ty, place);
        }
        Some(self.fresh(address, FeType::Struct(name)))
    }

    pub(crate) fn lower_enum_lit(
        &mut self,
        case_name: &klf_kira_syntax::ast::Ident,
        args: Option<&[Arg]>,
        expected: Option<&FeType>,
        span: Span,
    ) -> Option<(Name, FeType)> {
        let contextual = match expected {
            Some(FeType::Enum(enum_name)) => self.enums.get(enum_name).and_then(|info| {
                info.variant(case_name.name.as_str())
                    .map(|(index, case)| (enum_name.clone(), index, case))
            }),
            _ => None,
        };
        let matches: Vec<(Name, u32, crate::enums::EnumCase)> = if let Some(found) = contextual {
            vec![found]
        } else {
            self.enums
                .iter()
                .filter_map(|(enum_name, info)| {
                    info.variant(case_name.name.as_str())
                        .map(|(index, case)| (enum_name.clone(), index, case))
                })
                .collect()
        };
        let Some((enum_name, variant, case)) = matches.first().cloned() else {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0200",
                    format!("unknown enum case `{}`", case_name.name),
                )
                .with_label(span, "not found"),
            );
            return None;
        };
        if matches.len() > 1 {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0206",
                    format!("enum case `{}` is ambiguous", case_name.name),
                )
                .with_label(span, "qualify the enum case"),
            );
            return None;
        }
        let arguments = args.unwrap_or(&[]);
        if arguments.len() != case.payload.len() {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0202",
                    format!(
                        "case `{}` expects {} payload values, got {}",
                        case.name,
                        case.payload.len(),
                        arguments.len()
                    ),
                )
                .with_label(span, "wrong payload count"),
            );
            return None;
        }
        let address = self
            .fb
            .stack_alloc(Type::Nominal(enum_name.clone(), Vec::new()));
        if !case.payload.is_empty() {
            let payload = self.fb.enum_payload_addr(address.clone(), variant);
            let places = self.payload_places(&payload, case.payload.len());
            for ((argument, payload_ty), place) in arguments.iter().zip(&case.payload).zip(places) {
                let (value, value_ty) =
                    self.lower_expr_expected(&argument.value, Some(payload_ty))?;
                if !payload_ty.compatible(&value_ty) {
                    self.session.report(type_error(
                        argument.value.span,
                        format!(
                            "case payload is {}, got {}",
                            payload_ty.display(),
                            value_ty.display()
                        ),
                    ));
                    return None;
                }
                self.init_from(value, payload_ty, place);
            }
        }
        self.fb.enum_inject(address.clone(), variant);
        Some(self.fresh(address, FeType::Enum(enum_name)))
    }
}
