//! The `klf.editor.*` services that take text (`KLF-EDITOR.md` section 6),
//! called with a `String` as their last argument. Each lowers to its service
//! with the string as a (pointer, length) pair and is the service's result:
//! 1 when the fact was recorded, 0 otherwise, including in a build.
//!
//! The integer-only editor services are ordinary service wrappers
//! (`editor_classify`, `editor_symbol`, ...).

use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

/// Each builtin, its service, and how many integers precede the text.
const EDITOR_TEXT_BUILTINS: &[(&str, &str, usize)] = &[
    ("editorSymbolText", "klfsvc_editor_symbol_text", 2),
    ("editorTypeAt", "klfsvc_editor_type_at", 4),
    ("editorValueAt", "klfsvc_editor_value_at", 3),
    ("editorHint", "klfsvc_editor_hint", 5),
    ("editorLink", "klfsvc_editor_link", 5),
    ("editorRunnable", "klfsvc_editor_runnable", 4),
];

impl Lowerer<'_> {
    pub(crate) fn try_editor_builtin(&mut self, name: &str, args: &[Arg], span: Span) -> Builtin {
        let Some(&(builtin, symbol, integers)) = EDITOR_TEXT_BUILTINS
            .iter()
            .find(|(builtin, _, _)| *builtin == name)
        else {
            return Builtin::Skipped;
        };
        Builtin::from_lowered(self.lower_editor_text(builtin, symbol, integers, args, span))
    }

    fn lower_editor_text(
        &mut self,
        builtin: &str,
        symbol: &str,
        integers: usize,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let Some((text, leading)) = args.split_last().filter(|_| args.len() == integers + 1) else {
            self.session.report(type_error(
                span,
                format!("`{builtin}` takes {integers} Int arguments and then a String"),
            ));
            return None;
        };
        let mut arguments = Vec::with_capacity(integers + 2);
        for argument in leading {
            let (value, ty) = self.lower_expr(&argument.value)?;
            if ty != int64() {
                self.session.report(type_error(
                    argument.value.span,
                    format!("`{builtin}` expects an Int here"),
                ));
                return None;
            }
            arguments.push(value);
        }
        arguments.extend(self.string_argument(text, builtin)?);
        Some((self.fb.call_foreign(Name::new(symbol), arguments), int64()))
    }
}

fn int64() -> FeType {
    FeType::Int {
        bits: 64,
        signed: true,
    }
}
