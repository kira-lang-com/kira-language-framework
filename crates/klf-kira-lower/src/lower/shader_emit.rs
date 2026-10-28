//! The kernel entry around a `ShaderLanguage.emit` hook (`KLF-KSL.md`
//! section 5.8). The phase trampoline answers a word, so the lowerer emits
//! `__klf_shader_emit(unit) -> Int`, which widens the hook's `Bool`.

use super::compat_driver::entry_lowerer;
use klf_core::Name;
use klf_kir::{ConvertKind, Function, Linkage, Type};
use klf_services::{FunctionBuilder, Session};
use klf_source::Span;

pub(crate) const EMIT: &str = "emit";
const ENTRY: &str = "__klf_shader_emit";

pub(crate) fn lower_entry(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    span: Span,
) -> Function {
    let unit = Name::new("unit");
    let mut fb = FunctionBuilder::new(
        Name::new(ENTRY),
        &[(unit.clone(), Type::Integer(64))],
        Type::Integer(64),
        span,
    );
    fb.set_linkage(Linkage::Module);
    let mut lowerer = entry_lowerer(session, elaborated, counts, fb);
    let wrote = lowerer.fb.call(Name::new(EMIT), vec![unit]);
    let word = lowerer
        .fb
        .convert(ConvertKind::ZeroExtend, wrote, Type::Integer(64));
    lowerer.fb.ret(Some(word));
    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    function
}
