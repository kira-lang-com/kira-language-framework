//! The kernel entry around a `Language.compatDriver` override.
//!
//! The override takes `[String]` and returns `Optional<DriverRequest>`, neither
//! of which can cross the word-sized phase ABI. The lowerer therefore emits
//! `__klf_compat_driver(count) -> Int`, which rebuilds the argument list from
//! the `klf.driver.argument_*` services, calls the override, and hands a
//! `Some` result back one argument at a time through `klf.driver.request_push`.
//! It returns 0 for `None` and 1 for `Some`. Both the VM and a native frontend
//! call this entry, so the marshalling exists once, in KIR.

use super::Lowerer;
use crate::structs::DRIVER_REQUEST;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{BinaryOperator, BlockTarget, ConvertKind, Function, IntegerPredicate, Linkage};
use klf_kir::{Overflow, StoreMode, Type};
use klf_services::{FunctionBuilder, Session};
use klf_source::Span;

pub(crate) const COMPAT_DRIVER: &str = "compatDriver";
const ENTRY: &str = "__klf_compat_driver";

pub(crate) fn lower_entry(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    span: Span,
) -> Function {
    let count = Name::new("count");
    let mut fb = FunctionBuilder::new(
        Name::new(ENTRY),
        &[(count.clone(), Type::Integer(64))],
        Type::Integer(64),
        span,
    );
    fb.set_linkage(Linkage::Module);
    let mut lowerer = entry_lowerer(session, elaborated, counts, fb);
    let string = FeType::Struct(Name::new("String"));
    let arguments = lowerer.empty_list(&string);
    lowerer.counted_loop(&count, |lowerer, index| {
        let argument = lowerer.copy_argument(index);
        lowerer.list_push(&arguments, &string, argument);
    });

    let request = FeType::Struct(Name::new(DRIVER_REQUEST));
    let optional = crate::generics::instance_name("Optional", &[request]);
    let result = lowerer.fb.stack_alloc(Type::Nominal(optional, Vec::new()));
    lowerer
        .fb
        .call_void(Name::new(COMPAT_DRIVER), vec![result.clone(), arguments]);
    let none = lowerer.fb.new_block(&[]);
    let payload = lowerer.fb.fresh_value();
    let request_type = Type::Nominal(Name::new(DRIVER_REQUEST), Vec::new());
    let some = lowerer
        .fb
        .new_block(&[(payload.clone(), Type::Address(Box::new(request_type)))]);
    let payload_address = lowerer.fb.enum_payload_addr(result.clone(), 1);
    lowerer.fb.switch_enum(
        result,
        vec![
            (
                0,
                BlockTarget {
                    block: none.clone(),
                    arguments: Vec::new(),
                },
            ),
            (
                1,
                BlockTarget {
                    block: some.clone(),
                    arguments: vec![payload_address],
                },
            ),
        ],
        None,
    );

    lowerer.fb.switch_to(&none);
    let zero = lowerer.fb.const_int(Type::Integer(64), 0);
    lowerer.fb.ret(Some(zero));

    lowerer.fb.switch_to(&some);
    let field = lowerer
        .structs
        .get(&Name::new(DRIVER_REQUEST))
        .and_then(|info| info.field("arguments"))
        .map_or(0, |(index, _)| index);
    let normalized = lowerer.fb.field_addr(payload, field);
    let normalized_count = lowerer.list_count(&normalized);
    lowerer.counted_loop(&normalized_count, |lowerer, index| {
        let element = lowerer.list_element_addr(&normalized, &string, index);
        lowerer.push_request_argument(&element);
    });
    let one = lowerer.fb.const_int(Type::Integer(64), 1);
    lowerer.fb.ret(Some(one));

    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    function
}

/// A lowerer for a generated entry that returns an `Int`.
pub(super) fn entry_lowerer<'a>(
    session: &'a mut Session,
    elaborated: &'a crate::ElaboratedFile,
    counts: &'a std::cell::RefCell<super::ownership::NeededFunctions>,
    fb: FunctionBuilder,
) -> Lowerer<'a> {
    let int = FeType::Int {
        bits: 64,
        signed: true,
    };
    Lowerer::new(session, elaborated, counts, fb, int, None)
}

impl Lowerer<'_> {
    /// Runs `body` for every index in `0..count`.
    pub(super) fn counted_loop(&mut self, count: &Name, mut body: impl FnMut(&mut Self, Name)) {
        let counter = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());
        let header = self.fb.new_block(&[]);
        let step = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());

        self.fb.switch_to(&header);
        let index = self.fb.load(counter.clone());
        let more = self
            .fb
            .compare(IntegerPredicate::SignedLt, index, count.clone());
        self.fb
            .conditional_branch(more, step.clone(), Vec::new(), exit.clone(), Vec::new());

        self.fb.switch_to(&step);
        let index = self.fb.load(counter.clone());
        body(self, index.clone());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, index, one);
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(header, Vec::new());

        self.fb.switch_to(&exit);
    }

    /// Copies driver argument `index` into a fresh `String`.
    pub(super) fn copy_argument(&mut self, index: Name) -> Name {
        let length = self
            .fb
            .call_foreign(Name::new("klfsvc_driver_argument_len"), vec![index.clone()]);
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let valid = self
            .fb
            .compare(IntegerPredicate::SignedGe, length.clone(), zero);
        self.trap_unless(
            valid,
            "the driver has no argument at this index",
            "read `driverArgumentCount()` first and stay below it",
        );
        let bytes = self.fb.heap_alloc_array(Type::Integer(8), length.clone());
        let pointer = self
            .fb
            .convert(ConvertKind::Bitcast, bytes.clone(), Type::Pointer);
        let copied = self.fb.call_foreign(
            Name::new("klfsvc_driver_argument_copy"),
            vec![index, pointer, length.clone()],
        );
        let one = self.fb.const_int(Type::Integer(64), 1);
        let valid = self.fb.compare(IntegerPredicate::Eq, copied, one);
        self.trap_unless(
            valid,
            "the driver argument could not be copied",
            "the argument changed while it was read; this is a kernel bug, report it",
        );
        self.build_string(bytes, length)
    }

    fn push_request_argument(&mut self, string: &Name) {
        let bytes = self.string_bytes(string);
        let pointer = self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer);
        let length = self.string_len(string);
        let pushed = self.fb.call_foreign(
            Name::new("klfsvc_driver_request_push"),
            vec![pointer, length],
        );
        let one = self.fb.const_int(Type::Integer(64), 1);
        let valid = self.fb.compare(IntegerPredicate::Eq, pushed, one);
        self.trap_unless(
            valid,
            "the driver refused an argument the definition returned",
            "the driver takes UTF-8 text; check the argument for invalid bytes",
        );
    }

    /// Continues when `condition` holds, the answer of a kernel service the
    /// definition called, and otherwise traps with a report that says which
    /// service failed (`headline`) and what to check (`help`).
    pub(crate) fn trap_unless(&mut self, condition: Name, headline: &str, help: &str) {
        let pass = self.fb.new_block(&[]);
        let fail = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            condition,
            pass.clone(),
            Vec::new(),
            fail.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&fail);
        let place = self.fb.location().map(|location| klf_diag::report::Place {
            path: location.file.clone(),
            line: location.line as usize,
            column: location.column as usize,
            ..klf_diag::report::Place::default()
        });
        let report = klf_diag::report::Report {
            code: "KLF-RT-0007".to_owned(),
            headline: headline.to_owned(),
            place,
            notes: vec!["a kernel service the definition called answered with an error".to_owned()],
            helps: vec![help.to_owned()],
        };
        self.fb.trap(report.render());
        self.fb.switch_to(&pass);
    }
}
