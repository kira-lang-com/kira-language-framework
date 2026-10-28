use super::Lowerer;
use super::expr::Receiver;
use crate::ty::{DefinitionHandle, FeType};
use crate::type_error;
use klf_core::Name;
use klf_kir::{BlockTarget, ConvertKind, IntegerPredicate, StoreMode, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

impl Lowerer<'_> {
    pub(super) fn lower_session_flag(
        &mut self,
        receiver: Receiver<'_>,
        arguments: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [name_argument] = arguments else {
            self.session
                .report(type_error(span, "`Session.flag` takes one String argument"));
            return None;
        };
        let (session, receiver_type) = match receiver {
            Receiver::Expr(expression) => self.lower_expr(expression)?,
            Receiver::Path(path) => self.path_place(&path, span)?,
        };
        if receiver_type != FeType::DefinitionHandle(DefinitionHandle::Session) {
            self.session.report(type_error(
                span,
                "`.flag` is only available on a Session handle",
            ));
            return None;
        }
        let string_type = FeType::Struct(Name::new("String"));
        let (name, actual_type) =
            self.lower_expr_expected(&name_argument.value, Some(&string_type))?;
        if actual_type != string_type {
            self.session.report(type_error(
                name_argument.value.span,
                format!(
                    "`Session.flag` expects String, got {}",
                    actual_type.display()
                ),
            ));
            return None;
        }

        let name_bytes = self.string_bytes(&name);
        let name_pointer = self
            .fb
            .convert(ConvertKind::Bitcast, name_bytes, Type::Pointer);
        let name_length = self.string_len(&name);
        let output = self
            .fb
            .stack_alloc(Type::Array(2, Box::new(Type::Integer(64))));
        let zero_index = self.fb.const_int(Type::Integer(64), 0);
        let tag_address = self.fb.elem_addr(output.clone(), zero_index.clone());
        let tag_sentinel = self.fb.const_int(Type::Integer(64), -1);
        self.fb.store(StoreMode::Init, tag_sentinel, tag_address);
        let one_index = self.fb.const_int(Type::Integer(64), 1);
        let value_address = self.fb.elem_addr(output.clone(), one_index.clone());
        let zero = self.fb.const_int(Type::Integer(64), 0);
        self.fb.store(StoreMode::Init, zero.clone(), value_address);
        let output_pointer = self
            .fb
            .convert(ConvertKind::Bitcast, output.clone(), Type::Pointer);
        let _ = self.fb.call_foreign(
            Name::new("klfsvc_session_flag"),
            vec![
                session.clone(),
                name_pointer.clone(),
                name_length.clone(),
                output_pointer,
            ],
        );
        let tag_address = self.fb.elem_addr(output.clone(), zero_index);
        let tag = self.fb.load(tag_address);
        let value_address = self.fb.elem_addr(output, one_index);
        let payload = self.fb.load(value_address);
        Some(self.build_driver_flag_value(session, name_pointer, name_length, tag, payload))
    }

    fn build_driver_flag_value(
        &mut self,
        session: Name,
        name_pointer: Name,
        name_length: Name,
        tag: Name,
        payload: Name,
    ) -> (Name, FeType) {
        let enum_name = Name::new("DriverFlagValue");
        let result = self
            .fb
            .stack_alloc(Type::Nominal(enum_name.clone(), Vec::new()));
        let boolean = self.fb.new_block(&[]);
        let integer = self.fb.new_block(&[]);
        let string = self.fb.new_block(&[]);
        let invalid = self.fb.new_block(&[]);
        let done = self.fb.new_block(&[]);
        self.fb.switch_int(
            tag,
            vec![
                (
                    0,
                    BlockTarget {
                        block: boolean.clone(),
                        arguments: Vec::new(),
                    },
                ),
                (
                    1,
                    BlockTarget {
                        block: integer.clone(),
                        arguments: Vec::new(),
                    },
                ),
                (
                    2,
                    BlockTarget {
                        block: string.clone(),
                        arguments: Vec::new(),
                    },
                ),
            ],
            BlockTarget {
                block: invalid.clone(),
                arguments: Vec::new(),
            },
        );

        self.fb.switch_to(&boolean);
        let false_value = self.fb.const_int(Type::Integer(64), 0);
        let bool_value = self
            .fb
            .compare(IntegerPredicate::Ne, payload.clone(), false_value);
        let bool_payload = self.fb.enum_payload_addr(result.clone(), 0);
        self.fb.store(StoreMode::Init, bool_value, bool_payload);
        self.fb.enum_inject(result.clone(), 0);
        self.fb.branch(done.clone(), Vec::new());

        self.fb.switch_to(&integer);
        let int_payload = self.fb.enum_payload_addr(result.clone(), 1);
        self.fb.store(StoreMode::Init, payload.clone(), int_payload);
        self.fb.enum_inject(result.clone(), 1);
        self.fb.branch(done.clone(), Vec::new());

        self.fb.switch_to(&string);
        let bytes = self.fb.heap_alloc_array(Type::Integer(8), payload.clone());
        let bytes_pointer = self
            .fb
            .convert(ConvertKind::Bitcast, bytes.clone(), Type::Pointer);
        let copied = self.fb.call_foreign(
            Name::new("klfsvc_session_flag_string_copy"),
            vec![
                session,
                name_pointer,
                name_length,
                bytes_pointer,
                payload.clone(),
            ],
        );
        let success_value = self.fb.const_int(Type::Integer(64), 1);
        let success = self.fb.compare(IntegerPredicate::Eq, copied, success_value);
        let copied_block = self.fb.new_block(&[]);
        let copy_error = self.fb.new_block(&[]);
        self.fb.conditional_branch(
            success,
            copied_block.clone(),
            Vec::new(),
            copy_error.clone(),
            Vec::new(),
        );
        self.fb.switch_to(&copy_error);
        self.fb.trap("failed to copy driver flag String");
        self.fb.switch_to(&copied_block);
        let string_value = self.build_string(bytes, payload);
        let string_payload = self.fb.enum_payload_addr(result.clone(), 2);
        self.move_value(
            string_value,
            &FeType::Struct(Name::new("String")),
            string_payload,
        );
        self.fb.enum_inject(result.clone(), 2);
        self.fb.branch(done.clone(), Vec::new());

        self.fb.switch_to(&invalid);
        self.fb.trap("unknown driver flag");
        self.fb.switch_to(&done);
        self.reachable = true;
        (result, FeType::Enum(enum_name))
    }
}
