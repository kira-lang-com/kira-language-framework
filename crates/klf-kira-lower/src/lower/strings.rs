//! Builtin `String` lowering: literals and structural equality.

use super::Lowerer;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, IntegerPredicate, Overflow, StoreMode, Type};
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn lower_string_lit(&mut self, value: &str, span: Span) -> Option<(Name, FeType)> {
        let string_name = Name::new("String");
        let info = self.structs.get(&string_name)?.clone();
        let string_address = self
            .fb
            .stack_alloc(Type::Nominal(string_name.clone(), Vec::new()));
        let (data_index, data_ty) = info.field("data")?;
        let data_address = if value.is_empty() {
            let null = self.fb.const_null();
            self.fb
                .convert(ConvertKind::Bitcast, null, data_ty.to_kir())
        } else {
            // Read-only data the module carries: definitions never write a
            // string's bytes or free them, so every evaluation of the literal
            // can share them.
            let (bytes, _) = self.fb.const_string(value.to_owned());
            self.fb
                .convert(ConvertKind::Bitcast, bytes, data_ty.to_kir())
        };
        let data_place = self.fb.field_addr(string_address.clone(), data_index);
        self.fb.store(StoreMode::Init, data_address, data_place);
        let length = self
            .fb
            .const_int(Type::Integer(64), i128::try_from(value.len()).ok()?);
        let cap_index = info.field("cap")?.0;
        let len_index = info.field("len")?.0;
        let len_place = self.fb.field_addr(string_address.clone(), len_index);
        self.fb.store(StoreMode::Init, length.clone(), len_place);
        let cap_place = self.fb.field_addr(string_address.clone(), cap_index);
        self.fb.store(StoreMode::Init, length, cap_place);
        let ops_index = info.field("ops")?.0;
        let null = self.fb.const_null();
        let ops_place = self.fb.field_addr(string_address.clone(), ops_index);
        self.fb.store(StoreMode::Init, null, ops_place);
        let _ = span;
        Some((string_address, FeType::Struct(string_name)))
    }

    /// Structural equality for two builtin `String` values: equal lengths and
    /// byte-for-byte equal contents. Emits an inline compare loop and yields an
    /// `i1`; `negate` inverts the result for `!=`.
    pub(crate) fn lower_string_eq(&mut self, a: Name, b: Name, negate: bool) -> Name {
        let string_name = Name::new("String");
        let info = self.structs.get(&string_name).cloned();
        let data_index = info
            .as_ref()
            .and_then(|i| i.field("data"))
            .map_or(0, |(i, _)| i);
        let len_index = info
            .as_ref()
            .and_then(|i| i.field("len"))
            .map_or(1, |(i, _)| i);

        let a_len_addr = self.fb.field_addr(a.clone(), len_index);
        let a_len = self.fb.load(a_len_addr);
        let b_len_addr = self.fb.field_addr(b.clone(), len_index);
        let b_len = self.fb.load(b_len_addr);
        let len_eq = self.fb.compare(IntegerPredicate::Eq, a_len.clone(), b_len);
        let a_data_addr = self.fb.field_addr(a, data_index);
        let a_data = self.fb.load(a_data_addr);
        let b_data_addr = self.fb.field_addr(b, data_index);
        let b_data = self.fb.load(b_data_addr);
        let equal_val = self.fb.const_bool(!negate);
        let unequal_val = self.fb.const_bool(negate);
        let counter = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, counter.clone());

        let header = self.fb.new_block(&[]);
        let body = self.fb.new_block(&[]);
        let latch = self.fb.new_block(&[]);
        let result_param = self.fb.fresh_value();
        let join = self
            .fb
            .new_block(&[(result_param.clone(), Type::Integer(1))]);

        self.fb.conditional_branch(
            len_eq,
            header.clone(),
            Vec::new(),
            join.clone(),
            vec![unequal_val.clone()],
        );

        self.fb.switch_to(&header);
        let index = self.fb.load(counter.clone());
        let in_range = self
            .fb
            .compare(IntegerPredicate::SignedLt, index, a_len.clone());
        self.fb.conditional_branch(
            in_range,
            body.clone(),
            Vec::new(),
            join.clone(),
            vec![equal_val],
        );

        self.fb.switch_to(&body);
        let index = self.fb.load(counter.clone());
        let a_byte_addr = self.fb.elem_addr(a_data.clone(), index.clone());
        let a_byte = self.fb.load(a_byte_addr);
        let b_byte_addr = self.fb.elem_addr(b_data.clone(), index);
        let b_byte = self.fb.load(b_byte_addr);
        let byte_eq = self.fb.compare(IntegerPredicate::Eq, a_byte, b_byte);
        self.fb.conditional_branch(
            byte_eq,
            latch.clone(),
            Vec::new(),
            join.clone(),
            vec![unequal_val],
        );

        self.fb.switch_to(&latch);
        let index = self.fb.load(counter.clone());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let next = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, index, one);
        self.fb.store(StoreMode::Assign, next, counter);
        self.fb.branch(header, Vec::new());

        self.fb.switch_to(&join);
        result_param
    }
}
