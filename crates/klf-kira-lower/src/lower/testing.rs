//! `expect!(condition)`: hands the condition, its source text and its
//! `file:line:column` to `klf.test.expect`, which records an issue for the
//! running test when the condition is false.

use super::Lowerer;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{ConvertKind, Type};
use klf_kira_syntax::ast::{Arg, Expr, ExprKind};
use klf_source::Span;

impl Lowerer<'_> {
    pub(super) fn lower_expect(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if self.structs.get(&Name::new("TestSource")).is_none() {
            self.session
                .report(type_error(span, "`expect!` needs `import Testing`"));
            return None;
        }
        let [condition] = args else {
            self.session
                .report(type_error(span, "`expect!` takes one condition"));
            return None;
        };
        let (passed, ty) = self.lower_expr(&condition.value)?;
        if ty != FeType::Bool {
            self.session.report(type_error(
                condition.value.span,
                "`expect!` expects a Bool condition",
            ));
            return None;
        }
        let passed = self
            .fb
            .convert(ConvertKind::ZeroExtend, passed, Type::Integer(64));
        let file = self.session.file(condition.value.span.file);
        let text = file.text()
            [condition.value.span.start as usize..condition.value.span.end as usize]
            .to_owned();
        let place = file.line_column(span.start);
        let location = format!("{}:{}:{}", file.name(), place.line, place.column);
        let [text, text_length] = self.literal_argument(text, span)?;
        let [location, location_length] = self.literal_argument(location, span)?;
        self.fb.call_foreign_void(
            Name::new("klfsvc_test_expect"),
            vec![passed, text, text_length, location, location_length],
        );
        Some((self.fb.const_int(Type::Integer(64), 0), FeType::Void))
    }

    fn literal_argument(&mut self, text: String, span: Span) -> Option<[Name; 2]> {
        let literal = Arg {
            label: None,
            value: Expr {
                kind: ExprKind::Str(text),
                span,
            },
        };
        self.string_argument(&literal, "expect!")
    }
}
