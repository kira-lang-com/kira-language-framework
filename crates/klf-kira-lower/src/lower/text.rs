//! Host text prelude functions (`docs/host-contract.md`): `utf8Count`,
//! `utf8Byte`, `utf8Slice`, `joinText` and `decimalText`. They lower inline
//! over the builtin `String` representation rather than calling out to a
//! host; `host_text` holds the ones that read a host buffer.

use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, IntegerPredicate, Overflow, StoreMode, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

fn int64() -> FeType {
    FeType::Int {
        bits: 64,
        signed: true,
    }
}

pub(super) fn string_ty() -> FeType {
    FeType::Struct(Name::new("String"))
}

impl Lowerer<'_> {
    fn string_field(&self, name: &str) -> u32 {
        self.structs
            .get(&Name::new("String"))
            .and_then(|info| info.field(name))
            .map_or(0, |(index, _)| index)
    }

    /// A `String` header over `data` (a raw byte pointer) of `len` bytes.
    pub(super) fn build_string(&mut self, data: Name, len: Name) -> Name {
        let header = self
            .fb
            .stack_alloc(Type::Nominal(Name::new("String"), Vec::new()));
        let data_index = self.string_field("data");
        let len_index = self.string_field("len");
        let cap_index = self.string_field("cap");
        let ops_index = self.string_field("ops");
        let typed = Type::Address(Box::new(Type::Array(0, Box::new(Type::Integer(8)))));
        let data = self.fb.convert(ConvertKind::Bitcast, data, typed);
        let data_place = self.fb.field_addr(header.clone(), data_index);
        self.fb.store(StoreMode::Init, data, data_place);
        let len_place = self.fb.field_addr(header.clone(), len_index);
        self.fb.store(StoreMode::Init, len.clone(), len_place);
        let cap_place = self.fb.field_addr(header.clone(), cap_index);
        self.fb.store(StoreMode::Init, len, cap_place);
        let null = self.fb.const_null();
        let ops_place = self.fb.field_addr(header.clone(), ops_index);
        self.fb.store(StoreMode::Init, null, ops_place);
        header
    }

    /// The byte buffer of `string`, typed as an address to `[U8]`.
    pub(super) fn string_bytes(&mut self, string: &Name) -> Name {
        let data_index = self.string_field("data");
        let data_addr = self.fb.field_addr(string.clone(), data_index);
        let raw = self.fb.load(data_addr);
        let typed = Type::Address(Box::new(Type::Array(0, Box::new(Type::Integer(8)))));
        self.fb.convert(ConvertKind::Bitcast, raw, typed)
    }

    pub(super) fn string_len(&mut self, string: &Name) -> Name {
        let len_index = self.string_field("len");
        let addr = self.fb.field_addr(string.clone(), len_index);
        self.fb.load(addr)
    }

    /// Dispatch a host text builtin by name.
    pub(crate) fn try_text_builtin(&mut self, name: &str, args: &[Arg], span: Span) -> Builtin {
        let result = match name {
            "utf8Count" => self.lower_utf8_count(args, span),
            "utf8Byte" => self.lower_utf8_byte(args, span),
            "utf8Slice" => self.lower_utf8_slice(args, span),
            "joinText" => self.lower_join_text(args, span),
            "decimalText" => self.lower_decimal_text(args, span),
            "sourceText" => self.lower_source_text(args, span),
            "fileText" => self.lower_file_text(args, span),
            "fileName" => self.lower_file_name(args, span),
            "sourceName" => self.lower_source_name(args, span),
            "stdRoot" => self.lower_std_root(args, span),
            "targetTriple" => self.lower_target_triple(args, span),
            "driverDirectory" => self.lower_driver_directory(args, span),
            "shaderText" => self.lower_shader_text(args, span),
            "includeText" => self.lower_include_text(args, span),
            "includeExists" => self.lower_include_exists(args, span),
            _ => return Builtin::Skipped,
        };
        Builtin::from_lowered(result)
    }

    fn lower_utf8_count(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [arg] = args else {
            self.session
                .report(type_error(span, "`utf8Count` takes one argument"));
            return None;
        };
        let (string, _) = self.lower_expr(&arg.value)?;
        Some((self.string_len(&string), int64()))
    }

    fn lower_utf8_byte(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [text, offset] = args else {
            self.session
                .report(type_error(span, "`utf8Byte` takes two arguments"));
            return None;
        };
        let (string, _) = self.lower_expr(&text.value)?;
        let (index, _) = self.lower_expr(&offset.value)?;
        let bytes = self.string_bytes(&string);
        let place = self.fb.elem_addr(bytes, index);
        let byte = self.fb.load(place);
        let wide = self
            .fb
            .convert(ConvertKind::ZeroExtend, byte, Type::Integer(64));
        Some((wide, int64()))
    }

    fn lower_utf8_slice(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [text, start, end] = args else {
            self.session
                .report(type_error(span, "`utf8Slice` takes three arguments"));
            return None;
        };
        let (string, _) = self.lower_expr(&text.value)?;
        let (start, _) = self.lower_expr(&start.value)?;
        let (end, _) = self.lower_expr(&end.value)?;
        let len = self.fb.binary(
            BinaryOperator::Sub,
            Overflow::Wrap,
            true,
            end,
            start.clone(),
        );
        let buffer = self.fb.heap_alloc_array(Type::Integer(8), len.clone());
        let source = self.string_bytes(&string);
        self.copy_bytes_offset(&source, &start, &buffer, &len);
        let data = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        Some((self.build_string(data, len), string_ty()))
    }

    /// Copy `count` bytes from `source[offset..]` into `destination[0..]`.
    fn copy_bytes_offset(
        &mut self,
        source: &Name,
        offset: &Name,
        destination: &Name,
        count: &Name,
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
        let source_index = self.fb.binary(
            BinaryOperator::Add,
            Overflow::Wrap,
            true,
            offset.clone(),
            index.clone(),
        );
        let src = self.fb.elem_addr(source.clone(), source_index);
        let byte = self.fb.load(src);
        let dst = self.fb.elem_addr(destination.clone(), index);
        self.fb.store(StoreMode::Init, byte, dst);
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

    fn lower_join_text(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [arg] = args else {
            self.session
                .report(type_error(span, "`joinText` takes one argument"));
            return None;
        };
        let (list, list_ty) = self.lower_expr(&arg.value)?;
        if !matches!(list_ty, FeType::List(_)) {
            self.session
                .report(type_error(span, "`joinText` takes a list of strings"));
            return None;
        }
        let count = self.list_count(&list);
        let element = string_ty();
        let total = self.sum_string_lengths(&list, &element, &count);
        let buffer = self.fb.heap_alloc_array(Type::Integer(8), total.clone());
        self.concat_into(&list, &element, &count, &buffer);
        let data = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        Some((self.build_string(data, total), string_ty()))
    }

    fn sum_string_lengths(&mut self, list: &Name, element: &FeType, count: &Name) -> Name {
        let total = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, total.clone());
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
        let part = self.list_element_addr(list, element, index);
        let part_len = self.string_len(&part);
        let running = self.fb.load(total.clone());
        let updated = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, running, part_len);
        self.fb.store(StoreMode::Assign, updated, total.clone());
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
        self.fb.load(total)
    }

    fn concat_into(&mut self, list: &Name, element: &FeType, count: &Name, buffer: &Name) {
        let position = self.fb.stack_alloc(Type::Integer(64));
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero, position.clone());
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
        let part = self.list_element_addr(list, element, index);
        let part_bytes = self.string_bytes(&part);
        let part_len = self.string_len(&part);
        let start = self.fb.load(position.clone());
        self.copy_bytes_into(&part_bytes, buffer, &start, &part_len);
        let advanced = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, start, part_len);
        self.fb.store(StoreMode::Assign, advanced, position.clone());
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

    /// Copy `count` bytes from `source[0..]` into `destination[base..]`.
    fn copy_bytes_into(&mut self, source: &Name, destination: &Name, base: &Name, count: &Name) {
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
        let byte = self.fb.load(src);
        let dest_index = self.fb.binary(
            BinaryOperator::Add,
            Overflow::Wrap,
            true,
            base.clone(),
            index,
        );
        let dst = self.fb.elem_addr(destination.clone(), dest_index);
        self.fb.store(StoreMode::Init, byte, dst);
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

    fn lower_decimal_text(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [arg] = args else {
            self.session
                .report(type_error(span, "`decimalText` takes one argument"));
            return None;
        };
        let (value, _) = self.lower_expr(&arg.value)?;
        Some((self.emit_decimal_text(&value), string_ty()))
    }

    /// Base-10 signed formatting into a fresh `String`, no locale effects.
    fn emit_decimal_text(&mut self, value: &Name) -> Name {
        let (magnitude, sign_len, negative) = self.split_sign(value);
        let ten = self.fb.const_int(Type::Integer(64), 10);
        let digit_count = self.count_digits(&magnitude, &ten);
        let total = self.fb.binary(
            BinaryOperator::Add,
            Overflow::Wrap,
            true,
            digit_count,
            sign_len,
        );
        let buffer = self.fb.heap_alloc_array(Type::Integer(8), total.clone());
        self.emit_sign_byte(&negative, &buffer);
        self.write_digits(&buffer, &total, magnitude, &ten);
        let data = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        self.build_string(data, total)
    }

    /// Splits `value` into its magnitude and the length of its sign prefix,
    /// and hands back the predicate the sign byte is written under.
    fn split_sign(&mut self, value: &Name) -> (Name, Name, Name) {
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let negative = self
            .fb
            .compare(IntegerPredicate::SignedLt, value.clone(), zero.clone());
        let negated = self.fb.binary(
            BinaryOperator::Sub,
            Overflow::Wrap,
            true,
            zero,
            value.clone(),
        );
        let magnitude = self.fb.fresh_value();
        let sign_len = self.fb.fresh_value();
        let pick = self.fb.new_block(&[
            (magnitude.clone(), Type::Integer(64)),
            (sign_len.clone(), Type::Integer(64)),
        ]);
        let neg_block = self.fb.new_block(&[]);
        let pos_block = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            negative.clone(),
            neg_block.clone(),
            Vec::new(),
            pos_block.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&neg_block);
        let one = self.fb.const_int(Type::Integer(64), 1);
        self.fb.branch(pick.clone(), vec![negated, one]);
        self.fb.switch_to(&pos_block);
        let none = self.fb.const_int(Type::Integer(64), 0);
        self.fb.branch(pick.clone(), vec![value.clone(), none]);
        self.fb.switch_to(&pick);
        (magnitude, sign_len, negative)
    }

    /// The number of base-10 digits in `magnitude`, at least one. The
    /// magnitude is read unsigned: the negation of `Int`'s minimum wraps to
    /// itself, whose unsigned value is the magnitude, so its 19 digits are
    /// counted and written inside the buffer rather than before it.
    fn count_digits(&mut self, magnitude: &Name, ten: &Name) -> Name {
        let digits = self.fb.stack_alloc(Type::Integer(64));
        let one = self.fb.const_int(Type::Integer(64), 1);
        self.fb.store(StoreMode::Init, one, digits.clone());
        let rest = self.fb.stack_alloc(Type::Integer(64));
        let initial = self.fb.binary(
            BinaryOperator::UnsignedDiv,
            Overflow::Wrap,
            false,
            magnitude.clone(),
            ten.clone(),
        );
        self.fb.store(StoreMode::Init, initial, rest.clone());
        let header = self.fb.new_block(&[]);
        let body = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(header.clone(), Vec::new());
        self.fb.switch_to(&header);
        let current = self.fb.load(rest.clone());
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let more = self.fb.compare(IntegerPredicate::Ne, current, zero);
        self.fb
            .conditional_branch(more, body.clone(), Vec::new(), exit.clone(), Vec::new());
        self.fb.switch_to(&body);
        let digit_count = self.fb.load(digits.clone());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let bumped = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, digit_count, one);
        self.fb.store(StoreMode::Assign, bumped, digits.clone());
        let current = self.fb.load(rest.clone());
        let divided = self.fb.binary(
            BinaryOperator::UnsignedDiv,
            Overflow::Wrap,
            false,
            current,
            ten.clone(),
        );
        self.fb.store(StoreMode::Assign, divided, rest);
        self.fb.branch(header, Vec::new());
        self.fb.switch_to(&exit);
        self.fb.load(digits)
    }

    /// Writes the leading `-` when the value was negative.
    fn emit_sign_byte(&mut self, negative: &Name, buffer: &Name) {
        let dash_block = self.fb.new_block(&[]);
        let after_dash = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            negative.clone(),
            dash_block.clone(),
            Vec::new(),
            after_dash.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&dash_block);
        let dash = self.fb.const_int(Type::Integer(8), 45);
        let head = self.fb.const_int(Type::Integer(64), 0);
        let head_place = self.fb.elem_addr(buffer.clone(), head);
        self.fb.store(StoreMode::Init, dash, head_place);
        self.fb.branch(after_dash.clone(), Vec::new());
        self.fb.switch_to(&after_dash);
    }

    /// Fills `buffer` backwards from `total` with the digits of `magnitude`.
    fn write_digits(&mut self, buffer: &Name, total: &Name, magnitude: Name, ten: &Name) {
        let position = self.fb.stack_alloc(Type::Integer(64));
        self.fb
            .store(StoreMode::Init, total.clone(), position.clone());
        let remaining = self.fb.stack_alloc(Type::Integer(64));
        self.fb.store(StoreMode::Init, magnitude, remaining.clone());
        let body = self.fb.new_block(&[]);
        let exit = self.fb.new_block(&[]);
        self.fb.branch(body.clone(), Vec::new());
        self.fb.switch_to(&body);
        let position_value = self.fb.load(position.clone());
        let one = self.fb.const_int(Type::Integer(64), 1);
        let index = self.fb.binary(
            BinaryOperator::Sub,
            Overflow::Wrap,
            true,
            position_value,
            one,
        );
        self.fb.store(StoreMode::Assign, index.clone(), position);
        let value = self.fb.load(remaining.clone());
        let digit = self.fb.binary(
            BinaryOperator::UnsignedRem,
            Overflow::Wrap,
            false,
            value.clone(),
            ten.clone(),
        );
        let ascii = self.fb.const_int(Type::Integer(64), 48);
        let code = self
            .fb
            .binary(BinaryOperator::Add, Overflow::Wrap, true, digit, ascii);
        let byte = self
            .fb
            .convert(ConvertKind::Truncate, code, Type::Integer(8));
        let place = self.fb.elem_addr(buffer.clone(), index);
        self.fb.store(StoreMode::Init, byte, place);
        let next = self.fb.binary(
            BinaryOperator::UnsignedDiv,
            Overflow::Wrap,
            false,
            value,
            ten.clone(),
        );
        self.fb.store(StoreMode::Assign, next.clone(), remaining);
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let done = self.fb.compare(IntegerPredicate::Eq, next, zero);
        self.fb
            .conditional_branch(done, exit.clone(), Vec::new(), body.clone(), Vec::new());
        self.fb.switch_to(&exit);
    }
}
