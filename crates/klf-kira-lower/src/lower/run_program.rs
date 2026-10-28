//! The kernel entry around a `Language.runProgram` hook.
//!
//! `--emit run` builds the program, then hands the hook the command that runs
//! it and the driver arguments the build was asked with. Neither `[String]`
//! crosses the word-sized phase ABI, so the lowerer emits
//! `__klf_run_program(commandCount, total) -> Int`: the `klf.driver.argument_*`
//! services serve the command's arguments first and the driver arguments
//! after them, the entry rebuilds both lists, and the hook's `Int` is the
//! driver's exit status.

use super::compat_driver::entry_lowerer;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{BinaryOperator, Function, Linkage, Overflow, Type};
use klf_services::{FunctionBuilder, Session};
use klf_source::Span;

pub(crate) const RUN_PROGRAM: &str = "runProgram";
const ENTRY: &str = "__klf_run_program";

pub(crate) fn lower_entry(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    span: Span,
) -> Function {
    let command_count = Name::new("commandCount");
    let total = Name::new("total");
    let mut fb = FunctionBuilder::new(
        Name::new(ENTRY),
        &[
            (command_count.clone(), Type::Integer(64)),
            (total.clone(), Type::Integer(64)),
        ],
        Type::Integer(64),
        span,
    );
    fb.set_linkage(Linkage::Module);
    let mut lowerer = entry_lowerer(session, elaborated, counts, fb);
    let string = FeType::Struct(Name::new("String"));
    let command = lowerer.empty_list(&string);
    lowerer.counted_loop(&command_count, |lowerer, index| {
        let argument = lowerer.copy_argument(index);
        lowerer.list_push(&command, &string, argument);
    });
    let driver_arguments = lowerer.empty_list(&string);
    let driver_count = lowerer.fb.binary(
        BinaryOperator::Sub,
        Overflow::Wrap,
        true,
        total,
        command_count.clone(),
    );
    lowerer.counted_loop(&driver_count, |lowerer, index| {
        let at = lowerer.fb.binary(
            BinaryOperator::Add,
            Overflow::Wrap,
            true,
            index,
            command_count.clone(),
        );
        let argument = lowerer.copy_argument(at);
        lowerer.list_push(&driver_arguments, &string, argument);
    });
    let status = lowerer
        .fb
        .call(Name::new(RUN_PROGRAM), vec![command, driver_arguments]);
    lowerer.fb.ret(Some(status));
    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    function
}
