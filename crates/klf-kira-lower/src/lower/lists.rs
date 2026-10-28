//! Growable array (`[T]`) lowering. A list value is the address of a
//! `{count, cap, data}` header; `data` points at a run-time-sized
//! reference-counted buffer holding the elements contiguously.

use super::Lowerer;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, IntegerPredicate, Overflow, StoreMode, Type};
use klf_kira_syntax::ast::Expr;
use klf_source::Span;

pub(crate) fn is_aggregate(ty: &FeType) -> bool {
    matches!(
        ty,
        FeType::Struct(_)
            | FeType::Enum(_)
            | FeType::Array { .. }
            | FeType::List(_)
            | FeType::Function { .. }
    )
}

impl Lowerer<'_> {
    fn list_field(&self, name: &str) -> u32 {
        self.structs
            .get(&Name::new("List"))
            .and_then(|info| info.field(name))
            .map_or(0, |(index, _)| index)
    }

    /// The `(count, cap, data)` field indices of the `List` header.
    pub(crate) fn list_field_indices(&self) -> (u32, u32, u32) {
        (
            self.list_field("count"),
            self.list_field("cap"),
            self.list_field("data"),
        )
    }

    fn list_header(&mut self) -> Name {
        self.fb
            .stack_alloc(Type::Nominal(Name::new("List"), Vec::new()))
    }

    fn store_int_field(&mut self, header: &Name, field: u32, value: Name, mode: StoreMode) {
        let place = self.fb.field_addr(header.clone(), field);
        self.fb.store(mode, value, place);
    }

    /// The `data` pointer typed as an address to `[element]`, so `elem.addr`
    /// strides by the element size.
    fn list_data(&mut self, header: &Name, element: &FeType) -> Name {
        let data_index = self.list_field("data");
        let data_addr = self.fb.field_addr(header.clone(), data_index);
        let raw = self.fb.load(data_addr);
        let typed = Type::Address(Box::new(Type::Array(0, Box::new(element.to_kir()))));
        self.fb.convert(ConvertKind::Bitcast, raw, typed)
    }

    pub(crate) fn list_count(&mut self, header: &Name) -> Name {
        let count_index = self.list_field("count");
        let addr = self.fb.field_addr(header.clone(), count_index);
        self.fb.load(addr)
    }

    /// Address of `list[index]`, with the element stride from `element`.
    pub(crate) fn list_element_addr(
        &mut self,
        header: &Name,
        element: &FeType,
        index: Name,
    ) -> Name {
        let data = self.list_data(header, element);
        self.fb.elem_addr(data, index)
    }

    fn lower_empty_list(&mut self, element: &FeType) -> (Name, FeType) {
        let header = self.list_header();
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let count_index = self.list_field("count");
        let cap_index = self.list_field("cap");
        let data_index = self.list_field("data");
        self.store_int_field(&header, count_index, zero.clone(), StoreMode::Init);
        self.store_int_field(&header, cap_index, zero, StoreMode::Init);
        let null = self.fb.const_null();
        let data_place = self.fb.field_addr(header.clone(), data_index);
        self.fb.store(StoreMode::Init, null, data_place);
        (header, FeType::List(Box::new(element.clone())))
    }

    /// Build a list from element expressions. An empty literal needs an
    /// annotated element type from `expected`.
    pub(crate) fn lower_list_lit(
        &mut self,
        items: &[Expr],
        expected: Option<&FeType>,
        span: Span,
    ) -> Option<(Name, FeType)> {
        if items.is_empty() {
            let Some(FeType::List(element)) = expected else {
                self.session.report(type_error(
                    span,
                    "empty list needs an annotated element type",
                ));
                return None;
            };
            let (header, ty) = self.lower_empty_list(element);
            return Some(self.fresh(header, ty));
        }
        let expected_element = match expected {
            Some(FeType::List(element)) => Some((**element).clone()),
            _ => None,
        };
        let mut values = Vec::with_capacity(items.len());
        let mut element_ty: Option<FeType> = expected_element;
        for item in items {
            let (value, ty) = self.lower_expr(item)?;
            if let Some(expected) = &element_ty {
                if !expected.compatible(&ty) {
                    self.session.report(type_error(
                        item.span,
                        format!(
                            "list elements have types {} and {}",
                            expected.display(),
                            ty.display()
                        ),
                    ));
                    return None;
                }
            } else {
                element_ty = Some(ty.clone());
            }
            values.push(value);
        }
        let element = element_ty?;
        let length = i128::try_from(values.len()).ok()?;
        let header = self.list_header();
        let count_value = self.fb.const_int(Type::Integer(64), length);
        let count_index = self.list_field("count");
        let cap_index = self.list_field("cap");
        let data_index = self.list_field("data");
        self.store_int_field(&header, count_index, count_value.clone(), StoreMode::Init);
        self.store_int_field(&header, cap_index, count_value.clone(), StoreMode::Init);
        let buffer = self.fb.heap_alloc_array(element.to_kir(), count_value);
        let raw = self
            .fb
            .convert(ConvertKind::Bitcast, buffer.clone(), Type::Pointer);
        let data_place = self.fb.field_addr(header.clone(), data_index);
        self.fb.store(StoreMode::Init, raw, data_place);
        for (i, value) in values.into_iter().enumerate() {
            let index = self
                .fb
                .const_int(Type::Integer(64), i128::try_from(i).ok()?);
            let place = self.fb.elem_addr(buffer.clone(), index);
            self.init_from(value, &element, place);
        }
        Some(self.fresh(header, FeType::List(Box::new(element))))
    }

    /// `list.append(value)`: grow the buffer if full, store, bump `count`.
    pub(crate) fn lower_list_append(
        &mut self,
        header: &Name,
        element: &FeType,
        value_expr: &Expr,
    ) -> Option<(Name, FeType)> {
        let (value, value_ty) = self.lower_expr(value_expr)?;
        // An element of this very list would move with its buffer.
        let value = self.detach(value_expr, value, &value_ty);
        if !value_ty.compatible(element) {
            self.session.report(type_error(
                value_expr.span,
                format!(
                    "appending {} to a [{}]",
                    value_ty.display(),
                    element.display()
                ),
            ));
            return None;
        }
        self.list_push(header, element, value);
        Some((self.fb.const_int(Type::Integer(64), 0), FeType::Void))
    }

    pub(crate) fn list_push(&mut self, header: &Name, element: &FeType, value: Name) {
        self.list_make_room(header, element);
        let count = self.list_count(header);
        let place = self.list_element_addr(header, element, count.clone());
        self.init_from(value, element, place);
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, count, one);
        let count_index = self.list_field("count");
        self.store_int_field(header, count_index, next, StoreMode::Assign);
    }

    pub(crate) fn empty_list(&mut self, element: &FeType) -> Name {
        self.lower_empty_list(element).0
    }

    /// Makes room for one more element in a buffer no other list holds. A
    /// full or shared buffer is replaced: the elements move to the new one
    /// when this list was the buffer's only holder, and are copied, each
    /// taking its own counts, when another list still holds them.
    fn list_make_room(&mut self, header: &Name, element: &FeType) {
        self.list_own_buffer(header, element, true);
    }

    /// Gives the list a buffer no other list holds before one of its
    /// elements is written in place, so the write is not seen through a copy.
    pub(crate) fn list_make_unique(&mut self, header: &Name, element: &FeType) {
        self.list_own_buffer(header, element, false);
    }

    fn list_own_buffer(&mut self, header: &Name, element: &FeType, room: bool) {
        let count = self.list_count(header);
        let cap_index = self.list_field("cap");
        let cap_addr = self.fb.field_addr(header.clone(), cap_index);
        let cap = self.fb.load(cap_addr);
        let data_index = self.list_field("data");
        let buffer = self.buffer_of(header, data_index);
        let unique = self.buffer_is_unique(buffer.clone());
        let full = if room {
            self.fb
                .compare(IntegerPredicate::SignedGe, count.clone(), cap.clone())
        } else {
            self.fb.const_bool(false)
        };
        let yes = self.fb.const_bool(true);
        let shared = self.fb.binary(
            BinaryOperator::Xor,
            Overflow::Wrap,
            false,
            unique.clone(),
            yes,
        );
        let replace = self.fb.binary(
            BinaryOperator::Or,
            Overflow::Wrap,
            false,
            full.clone(),
            shared,
        );
        let grow = self.fb.new_block(&[]);
        let done = self.fb.new_block(&[]);
        self.fb
            .conditional_branch(replace, grow.clone(), Vec::new(), done.clone(), Vec::new());

        self.fb.switch_to(&grow);
        // A full buffer doubles (plus four); a shared one keeps its capacity.
        let two = self.fb.const_int(Type::Integer(64), 2);
        let doubled = self
            .fb
            .binary(BinaryOperator::Mul, Overflow::Wrap, true, cap.clone(), two);
        let four = self.fb.const_int(Type::Integer(64), 4);
        let grown = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, doubled, four);
        let extra = self.fb.binary(
            BinaryOperator::Sub,
            Overflow::Wrap,
            true,
            grown,
            cap.clone(),
        );
        let full_wide = self
            .fb
            .convert(ConvertKind::ZeroExtend, full, Type::Integer(64));
        let added = self
            .fb
            .binary(BinaryOperator::Mul, Overflow::Wrap, true, full_wide, extra);
        let newcap = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, cap, added);
        let fresh = self.fb.heap_alloc_array(element.to_kir(), newcap.clone());
        let old = self.list_data(header, element);
        if self.counted(element) {
            let moved = self.fb.new_block(&[]);
            let copied = self.fb.new_block(&[]);
            let filled = self.fb.new_block(&[]);
            self.fb.conditional_branch(
                unique,
                moved.clone(),
                Vec::new(),
                copied.clone(),
                Vec::new(),
            );
            self.fb.switch_to(&moved);
            self.list_copy_prefix(&old, &fresh, element, &count, false);
            self.fb.branch(filled.clone(), Vec::new());
            self.fb.switch_to(&copied);
            self.list_copy_prefix(&old, &fresh, element, &count, true);
            self.fb.branch(filled.clone(), Vec::new());
            self.fb.switch_to(&filled);
        } else {
            self.list_copy_prefix(&old, &fresh, element, &count, false);
        }
        self.count_buffer(super::ownership::Count::Release, buffer);
        let raw = self.fb.convert(ConvertKind::Bitcast, fresh, Type::Pointer);
        let data_place = self.fb.field_addr(header.clone(), data_index);
        self.fb.store(StoreMode::Assign, raw, data_place);
        self.store_int_field(header, cap_index, newcap, StoreMode::Assign);
        self.fb.branch(done.clone(), Vec::new());

        self.fb.switch_to(&done);
    }

    /// Copy `count` elements from `source` to `destination` (both typed
    /// buffers); with `counting`, each copy takes its own counts.
    fn list_copy_prefix(
        &mut self,
        source: &Name,
        destination: &Name,
        element: &FeType,
        count: &Name,
        counting: bool,
    ) {
        let counter = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());
        let header = self.fb.new_block(&[]);
        let body = self.fb.new_block(&[]);
        let latch = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());
        self.fb.switch_to(&header);
        let index = self.fb.load(counter.clone());
        let more = self
            .fb
            .compare(IntegerPredicate::SignedLt, index, count.clone());
        self.fb
            .conditional_branch(more, body.clone(), Vec::new(), exit.clone(), Vec::new());
        self.fb.switch_to(&body);
        let index = self.fb.load(counter.clone());
        let src = self.fb.elem_addr(source.clone(), index.clone());
        let dst = self.fb.elem_addr(destination.clone(), index);
        let value = if is_aggregate(element) {
            src
        } else {
            self.fb.load(src)
        };
        if counting {
            self.store_value(StoreMode::Init, value, element, dst);
        } else {
            self.copy_bits(StoreMode::Init, value, element, dst);
        }
        self.fb.branch(latch.clone(), Vec::new());
        self.fb.switch_to(&latch);
        let index = self.fb.load(counter.clone());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, index, one);
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(header, Vec::new());
        self.fb.switch_to(&exit);
    }
}
