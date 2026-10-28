//! The builtins the generated `__klf_command` entry reads parsed values
//! through (`KLF-CLI.md` section 4): `__klf_cli_count(slot)`, the number of
//! values an argument slot holds; `__klf_cli_int(slot, index)`, one value as
//! an integer (a Bool as 0 or 1, a choice as its index, a counted flag as its
//! count); and `__klf_cli_string(slot, index)`, one value as a String.

use super::text::string_ty;
use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn try_cli_builtin(&mut self, name: &str, args: &[Arg], span: Span) -> Builtin {
        let arity = match name {
            "__klf_cli_count" => 1,
            "__klf_cli_int" | "__klf_cli_string" => 2,
            _ => return Builtin::Skipped,
        };
        if args.len() != arity {
            self.session.report(type_error(
                span,
                format!("`{name}` takes {arity} Int arguments"),
            ));
            return Builtin::Failed;
        }
        let mut words = Vec::with_capacity(arity);
        for arg in args {
            let Some((word, _)) = self.lower_expr(&arg.value) else {
                return Builtin::Failed;
            };
            words.push(word);
        }
        let int = FeType::Int {
            bits: 64,
            signed: true,
        };
        match name {
            "__klf_cli_count" => {
                let count = self.fb.call_foreign(Name::new("klfsvc_cli_count"), words);
                Builtin::Lowered(count, int)
            }
            "__klf_cli_int" => {
                let value = self.fb.call_foreign(Name::new("klfsvc_cli_int"), words);
                Builtin::Lowered(value, int)
            }
            _ => {
                let (text, _) = self.read_source_buffer(
                    "klfsvc_cli_string_len",
                    "klfsvc_cli_string_copy",
                    words,
                );
                Builtin::Lowered(text, string_ty())
            }
        }
    }
}
