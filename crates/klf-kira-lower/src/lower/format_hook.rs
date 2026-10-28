//! The kernel entry around a `format` hook (`KLF-FORMAT.md` section 3). The
//! native trampoline answers a word, so the lowerer emits
//! `__klf_format(session, unit, tree) -> Int`, which widens the hook's `Bool`.

use super::compat_driver::entry_lowerer;
use klf_core::Name;
use klf_kir::{ConvertKind, Function, Linkage, Type};
use klf_services::{FunctionBuilder, Session};
use klf_source::Span;

pub(crate) const FORMAT: &str = "format";
const ENTRY: &str = "__klf_format";

pub(crate) fn lower_entry(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    span: Span,
) -> Function {
    let parameters = [Name::new("session"), Name::new("unit"), Name::new("tree")];
    let mut fb = FunctionBuilder::new(
        Name::new(ENTRY),
        &parameters
            .iter()
            .map(|name| (name.clone(), Type::Integer(64)))
            .collect::<Vec<_>>(),
        Type::Integer(64),
        span,
    );
    fb.set_linkage(Linkage::Module);
    let mut lowerer = entry_lowerer(session, elaborated, counts, fb);
    let formatted = lowerer.fb.call(Name::new(FORMAT), parameters.to_vec());
    let word = lowerer
        .fb
        .convert(ConvertKind::ZeroExtend, formatted, Type::Integer(64));
    lowerer.fb.ret(Some(word));
    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    function
}
