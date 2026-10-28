use super::Lowerer;
use super::expr::Receiver;
use crate::ty::{DefinitionHandle, FeType};
use crate::type_error;
use klf_core::Name;
use klf_kir::{BlockTarget, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

struct FileEnum {
    method: &'static str,
    service: &'static str,
    enum_name: &'static str,
    variant_count: usize,
}

impl Lowerer<'_> {
    pub(super) fn lower_session_file_memory(
        &mut self,
        receiver: Receiver<'_>,
        arguments: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        self.lower_session_file_enum(
            receiver,
            arguments,
            span,
            &FileEnum {
                method: "file_memory",
                service: "klfsvc_session_file_memory",
                enum_name: "MemoryModel",
                variant_count: 3,
            },
        )
    }

    pub(super) fn lower_session_file_convention(
        &mut self,
        receiver: Receiver<'_>,
        arguments: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        self.lower_session_file_enum(
            receiver,
            arguments,
            span,
            &FileEnum {
                method: "file_convention",
                service: "klfsvc_session_file_convention",
                enum_name: "DefaultConvention",
                variant_count: 2,
            },
        )
    }

    fn lower_session_file_enum(
        &mut self,
        receiver: Receiver<'_>,
        arguments: &[Arg],
        span: Span,
        file_enum: &FileEnum,
    ) -> Option<(Name, FeType)> {
        let FileEnum {
            method,
            service,
            enum_name,
            variant_count,
        } = *file_enum;
        let [file_argument] = arguments else {
            self.session.report(type_error(
                span,
                format!("`Session.{method}` takes one SourceFile argument"),
            ));
            return None;
        };
        let (session, receiver_type) = match receiver {
            Receiver::Expr(expression) => self.lower_expr(expression)?,
            Receiver::Path(path) => self.path_place(&path, span)?,
        };
        if receiver_type != FeType::DefinitionHandle(DefinitionHandle::Session) {
            self.session.report(type_error(
                span,
                format!("`.{method}` is only available on a Session handle"),
            ));
            return None;
        }
        let source_file_type = FeType::DefinitionHandle(DefinitionHandle::SourceFile);
        let (file, actual_type) =
            self.lower_expr_expected(&file_argument.value, Some(&source_file_type))?;
        if actual_type != source_file_type {
            self.session.report(type_error(
                file_argument.value.span,
                format!(
                    "`Session.{method}` expects SourceFile, got {}",
                    actual_type.display()
                ),
            ));
            return None;
        }

        let tag = self
            .fb
            .call_foreign(Name::new(service), vec![session, file]);
        let enum_name = Name::new(enum_name);
        let value = self
            .fb
            .stack_alloc(Type::Nominal(enum_name.clone(), Vec::new()));
        let variants: Vec<_> = (0..variant_count).map(|_| self.fb.new_block(&[])).collect();
        let invalid = self.fb.new_block(&[]);
        let done = self.fb.new_block(&[]);
        let cases = variants
            .iter()
            .enumerate()
            .map(|(index, block)| {
                (
                    i128::try_from(index).expect("builtin enum tag fits i128"),
                    BlockTarget {
                        block: block.clone(),
                        arguments: Vec::new(),
                    },
                )
            })
            .collect();
        self.fb.switch_int(
            tag,
            cases,
            BlockTarget {
                block: invalid.clone(),
                arguments: Vec::new(),
            },
        );
        for (index, block) in variants.iter().enumerate() {
            self.fb.switch_to(block);
            self.fb.enum_inject(
                value.clone(),
                u32::try_from(index).expect("builtin enum variant count fits u32"),
            );
            self.fb.branch(done.clone(), Vec::new());
        }
        self.fb.switch_to(&invalid);
        self.fb.trap(format!("invalid {method} service result"));
        self.fb.switch_to(&done);
        self.reachable = true;
        Some((value, FeType::Enum(enum_name)))
    }

    pub(super) fn lower_session_file_mode(
        &mut self,
        receiver: Receiver<'_>,
        arguments: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [file_argument] = arguments else {
            self.session.report(type_error(
                span,
                "`Session.file_mode` takes one SourceFile argument",
            ));
            return None;
        };
        let (session, receiver_type) = match receiver {
            Receiver::Expr(expression) => self.lower_expr(expression)?,
            Receiver::Path(path) => self.path_place(&path, span)?,
        };
        if receiver_type != FeType::DefinitionHandle(DefinitionHandle::Session) {
            self.session.report(type_error(
                span,
                "`.file_mode` is only available on a Session handle",
            ));
            return None;
        }
        let source_file_type = FeType::DefinitionHandle(DefinitionHandle::SourceFile);
        let (file, actual_type) =
            self.lower_expr_expected(&file_argument.value, Some(&source_file_type))?;
        if actual_type != source_file_type {
            self.session.report(type_error(
                file_argument.value.span,
                format!(
                    "`Session.file_mode` expects SourceFile, got {}",
                    actual_type.display()
                ),
            ));
            return None;
        }
        let value = self
            .fb
            .call_foreign(Name::new("klfsvc_session_file_mode"), vec![session, file]);
        Some((
            value,
            FeType::Int {
                bits: 64,
                signed: true,
            },
        ))
    }
}
