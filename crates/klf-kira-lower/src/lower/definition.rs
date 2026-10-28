//! The `Language` phase ABI (section 7.2) on the KLF Kira side.
//!
//! A phase declares the signature the specification gives it, but the words it
//! exchanges with the kernel are handles. Three pieces of that translation are
//! emitted here rather than written by the definition:
//!
//!   * the `[SourceFile]` argument of `units`, rebuilt in the prologue from the
//!     input count and `klf.definition.input`;
//!   * the `[Unit]` result of `units`, pushed one handle at a time and closed
//!     with `klf.definition.units_finish`;
//!   * `artifactHandle` and `artifactValue`, which park a definition-owned
//!     value where the next phase can still reach it and read it back.

use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, IntegerPredicate, Overflow, StoreMode, Type};
use klf_kira_syntax::ast::{Arg, Expr, ExprKind};
use klf_source::Span;

impl Lowerer<'_> {
    /// Materializes the `[SourceFile]` argument of `units` as a real list.
    ///
    /// The phase ABI passes only the number of inputs; each handle is read back
    /// through `klf.definition.input` so one unit can be built per file.
    pub(crate) fn build_input_list(
        &mut self,
        session: &Name,
        count: &Name,
        element: &FeType,
    ) -> Name {
        let header = self
            .fb
            .stack_alloc(Type::Nominal(Name::new("List"), Vec::new()));
        let fields = self.list_field_indices();
        let count_place = self.fb.field_addr(header.clone(), fields.0);
        self.fb.store(StoreMode::Init, count.clone(), count_place);
        let cap_place = self.fb.field_addr(header.clone(), fields.1);
        self.fb.store(StoreMode::Init, count.clone(), cap_place);
        let buffer = self.fb.heap_alloc_array(element.to_kir(), count.clone());
        let raw = self
            .fb
            .convert(ConvertKind::Bitcast, buffer.clone(), Type::Pointer);
        let data_place = self.fb.field_addr(header.clone(), fields.2);
        self.fb.store(StoreMode::Init, raw, data_place);

        let counter = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());
        let test = self.fb.new_block(&[]);
        let body = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(test.clone(), Vec::new());
        self.fb.switch_to(&test);
        let index = self.fb.load(counter.clone());
        let more = self
            .fb
            .compare(IntegerPredicate::SignedLt, index, count.clone());
        self.fb
            .conditional_branch(more, body.clone(), Vec::new(), exit.clone(), Vec::new());
        self.fb.switch_to(&body);
        let index = self.fb.load(counter.clone());
        let file = self.fb.call_foreign(
            Name::new("klfsvc_definition_input"),
            vec![session.clone(), index.clone()],
        );
        let place = self.fb.elem_addr(buffer, index.clone());
        self.fb.store(StoreMode::Init, file, place);
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, index, one);
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(test, Vec::new());
        self.fb.switch_to(&exit);
        header
    }

    /// Returns the `[Unit]` result of `units` as one unit-list artifact.
    pub(crate) fn return_unit_list(&mut self, session: &Name, list: &Name) {
        let count = self.list_count(list);
        let element = FeType::DefinitionHandle(crate::ty::DefinitionHandle::Unit);
        let counter = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());
        let test = self.fb.new_block(&[]);
        let body = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(test.clone(), Vec::new());
        self.fb.switch_to(&test);
        let index = self.fb.load(counter.clone());
        let more = self
            .fb
            .compare(IntegerPredicate::SignedLt, index, count.clone());
        self.fb
            .conditional_branch(more, body.clone(), Vec::new(), exit.clone(), Vec::new());
        self.fb.switch_to(&body);
        let index = self.fb.load(counter.clone());
        let place = self.list_element_addr(list, &element, index.clone());
        let unit = self.fb.load(place);
        self.fb.call_foreign_void(
            Name::new("klfsvc_definition_units_push"),
            vec![session.clone(), unit],
        );
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, index, one);
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(test, Vec::new());
        self.fb.switch_to(&exit);
        let artifact = self.fb.call_foreign(
            Name::new("klfsvc_definition_units_finish"),
            vec![session.clone()],
        );
        self.release_all();
        self.fb.ret(Some(artifact));
    }

    /// `artifactHandle(value)`: copies an aggregate out of the frame that built
    /// it and yields the word that addresses the copy.
    ///
    /// A phase result outlives the call that produced it, so the value cannot
    /// stay in a stack slot. The copy goes to the definition heap, which lives
    /// as long as the machine running the phases of one unit, as a raw block:
    /// only the kernel holds its address once the phase returns, so it must
    /// not be a collected block that a collection could find unreached. Its
    /// size is the stride `elem.addr` steps by from a null base.
    pub(crate) fn lower_artifact_handle(
        &mut self,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [argument] = args else {
            self.session
                .report(type_error(span, "`artifactHandle` takes one argument"));
            return None;
        };
        let (value, ty) = self.lower_expr(&argument.value)?;
        if !super::lists::is_aggregate(&ty) {
            self.session.report(type_error(
                span,
                format!("`artifactHandle` needs an aggregate, not {}", ty.display()),
            ));
            return None;
        }
        let array = Type::Address(Box::new(Type::Array(0, Box::new(ty.to_kir()))));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let one = self.fb.const_int(Type::Integer(64), 1);
        let null = self
            .fb
            .convert(ConvertKind::Bitcast, zero.clone(), array.clone());
        let end = self.fb.elem_addr(null, one);
        let size = self
            .fb
            .convert(ConvertKind::Bitcast, end, Type::Integer(64));
        let align = self.fb.const_int(Type::Integer(64), 16);
        let raw = self.fb.heap_alloc_raw(size, align);
        let buffer = self.fb.convert(ConvertKind::Bitcast, raw, array);
        let place = self.fb.elem_addr(buffer, zero);
        self.store_value(StoreMode::Init, value, &ty, place.clone());
        let handle = self
            .fb
            .convert(ConvertKind::Bitcast, place, Type::Integer(64));
        Some((
            handle,
            FeType::Int {
                bits: 64,
                signed: true,
            },
        ))
    }

    /// `artifactValue(handle)`: the aggregate an earlier phase parked, typed by
    /// the annotation at the use site.
    pub(crate) fn try_artifact_value(&mut self, expr: &Expr, expected: Option<&FeType>) -> Builtin {
        let ExprKind::Call { callee, args } = &expr.kind else {
            return Builtin::Skipped;
        };
        let ExprKind::Path(path) = &callee.kind else {
            return Builtin::Skipped;
        };
        if path.segments.len() != 1 || path.segments[0].name.as_str() != "artifactValue" {
            return Builtin::Skipped;
        }
        let Some(expected) = expected.filter(|ty| super::lists::is_aggregate(ty)) else {
            self.session.report(type_error(
                expr.span,
                "`artifactValue` needs an annotated aggregate type",
            ));
            return Builtin::Failed;
        };
        let expected = expected.clone();
        let [argument] = args.as_slice() else {
            self.session
                .report(type_error(expr.span, "`artifactValue` takes one argument"));
            return Builtin::Failed;
        };
        let Some((handle, _)) = self.lower_expr(&argument.value) else {
            return Builtin::Failed;
        };
        let address = self
            .fb
            .convert(ConvertKind::Bitcast, handle, expected.value_kir());
        Builtin::Lowered(address, expected)
    }
}
