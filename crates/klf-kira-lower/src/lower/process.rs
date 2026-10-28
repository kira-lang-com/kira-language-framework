//! `runCommand`, `runAttached`, `commandOutput`, `writeDriverOutput` and
//! `writeDriverError`: a definition hook that runs a built program reaches
//! the `klf.process` command services and the driver's own output through
//! these.
//!
//! `runCommand(command)` runs the `[String]` command, whose first element is
//! the program, to its end with its output captured, and is its exit status:
//! -1 when it ended without one, -2 when it could not start.
//! `runAttached(command)` runs it the same way on the driver's own input and
//! output instead. `commandOutput(stream)` is what the last command wrote to
//! stream 1, its output, or 2, its errors. `writeDriverOutput(text)` writes
//! `text` to the driver's output and `writeDriverError(text)` to its error
//! output; each is the count written.

use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{ConvertKind, IntegerPredicate, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn try_process_builtin(&mut self, name: &str, args: &[Arg], span: Span) -> Builtin {
        let result = match name {
            "runCommand" => self.lower_run_command(args, span, "runCommand", true),
            "runAttached" => self.lower_run_command(args, span, "runAttached", false),
            "commandOutput" => self.lower_command_output(args, span),
            "writeOutput" | "writeDriverOutput" => {
                self.lower_write(args, span, "writeOutput", "klfsvc_io_write")
            }
            "writeError" | "writeDriverError" => {
                self.lower_write(args, span, "writeError", "klfsvc_io_write_error")
            }
            _ => return Builtin::Skipped,
        };
        Builtin::from_lowered(result)
    }

    fn lower_run_command(
        &mut self,
        args: &[Arg],
        span: Span,
        builtin: &str,
        captured: bool,
    ) -> Option<(Name, FeType)> {
        let [command] = args else {
            self.session.report(type_error(
                span,
                format!("`{builtin}` takes the command to run"),
            ));
            return None;
        };
        let (command, ty) = self.lower_expr(&command.value)?;
        let string = FeType::Struct(Name::new("String"));
        if ty != FeType::List(Box::new(string.clone())) {
            self.session.report(type_error(
                span,
                format!("`{builtin}` expects a [String] command"),
            ));
            return None;
        }
        let count = self.list_count(&command);
        self.counted_loop(&count, |lowerer, index| {
            let argument = lowerer.list_element_addr(&command, &string, index);
            let bytes = lowerer.string_bytes(&argument);
            let pointer = lowerer
                .fb
                .convert(ConvertKind::Bitcast, bytes, Type::Pointer);
            let length = lowerer.string_len(&argument);
            let pushed = lowerer.fb.call_foreign(
                Name::new("klfsvc_process_command_push"),
                vec![pointer, length],
            );
            let one = lowerer.fb.const_int(Type::Integer(64), 1);
            let valid = lowerer.fb.compare(IntegerPredicate::Eq, pushed, one);
            lowerer.trap_unless(
                valid,
                "a command cannot take this argument",
                "an argument must be UTF-8 text with no NUL byte",
            );
        });
        let captured = self.fb.const_int(Type::Integer(64), i128::from(captured));
        let status = self
            .fb
            .call_foreign(Name::new("klfsvc_process_command_run"), vec![captured]);
        Some((status, int64()))
    }

    fn lower_command_output(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [stream] = args else {
            self.session
                .report(type_error(span, "`commandOutput` takes a stream, 1 or 2"));
            return None;
        };
        let (stream, _) = self.lower_expr(&stream.value)?;
        let output = Name::new("klfsvc_process_output");
        let null = self.fb.const_null();
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let length = self
            .fb
            .call_foreign(output.clone(), vec![stream.clone(), null, zero.clone()]);
        let valid = self
            .fb
            .compare(IntegerPredicate::SignedGe, length.clone(), zero);
        self.trap_unless(
            valid,
            "`commandOutput` reads stream 1 or 2",
            "pass 1 for the command's standard output or 2 for its standard error",
        );
        let bytes = self.fb.heap_alloc_array(Type::Integer(8), length.clone());
        let pointer = self
            .fb
            .convert(ConvertKind::Bitcast, bytes.clone(), Type::Pointer);
        let copied = self
            .fb
            .call_foreign(output, vec![stream, pointer, length.clone()]);
        let whole = self
            .fb
            .compare(IntegerPredicate::Eq, copied, length.clone());
        self.trap_unless(
            whole,
            "the command's output changed while it was read",
            "read the output once the command has finished, after `runCommand` returns",
        );
        Some((
            self.build_string(bytes, length),
            FeType::Struct(Name::new("String")),
        ))
    }

    fn lower_write(
        &mut self,
        args: &[Arg],
        span: Span,
        builtin: &str,
        service: &str,
    ) -> Option<(Name, FeType)> {
        let [text] = args else {
            self.session.report(type_error(
                span,
                format!("`{builtin}` takes the text to write"),
            ));
            return None;
        };
        let arguments = self.string_argument(text, builtin)?;
        let written = self.fb.call_foreign(Name::new(service), arguments.to_vec());
        Some((written, int64()))
    }
}

fn int64() -> FeType {
    FeType::Int {
        bits: 64,
        signed: true,
    }
}
