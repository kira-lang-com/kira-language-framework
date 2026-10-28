//! The text builtins that read a buffer the host holds: `sourceText`,
//! `fileText`, `fileName`, `sourceName`, `stdRoot`, `targetTriple`, `driverDirectory`,
//! `includeText`, `includeExists` and `shaderText`. The service ABI carries
//! words only, so each asks the buffer's length, allocates, and has the host
//! copy into it.

use super::Lowerer;
use super::text::string_ty;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{ConvertKind, IntegerPredicate, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

impl Lowerer<'_> {
    /// The whole text of the unit under compilation, read through
    /// `klf.source.len` and `klf.source.byte`.
    pub(super) fn lower_source_text(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if !args.is_empty() {
            self.session
                .report(type_error(span, "`sourceText` takes no arguments"));
            return None;
        }
        Some(self.read_source_buffer("klfsvc_source_len", "klfsvc_source_copy", Vec::new()))
    }

    /// The whole text of one of the compilation's input files, read through
    /// `klf.source.file_len` and `klf.source.file_copy`. A phase runs on one
    /// unit; this is how it reads another one to check the whole program.
    pub(super) fn lower_file_text(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, file] = args else {
            self.session
                .report(type_error(span, "`fileText` takes a session and a file"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let (file, _) = self.lower_expr(&file.value)?;
        Some(self.read_source_buffer(
            "klfsvc_source_file_len",
            "klfsvc_source_file_copy",
            vec![session, file],
        ))
    }

    pub(super) fn lower_file_name(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, file] = args else {
            self.session
                .report(type_error(span, "`fileName` takes a session and a file"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let (file, _) = self.lower_expr(&file.value)?;
        Some(self.read_source_buffer(
            "klfsvc_source_file_name_len",
            "klfsvc_source_file_name_copy",
            vec![session, file],
        ))
    }

    /// The directory of the language's std sources (D-327), read through
    /// `klf.session.std_root_len` and `klf.session.std_root_copy`; empty
    /// when the driver knows none.
    pub(super) fn lower_std_root(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session] = args else {
            self.session
                .report(type_error(span, "`stdRoot` takes a session"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        Some(self.read_source_buffer(
            "klfsvc_session_std_root_len",
            "klfsvc_session_std_root_copy",
            vec![session],
        ))
    }

    /// The triple the session compiles for, read through
    /// `klf.session.target_len` and `klf.session.target_copy`.
    pub(super) fn lower_target_triple(
        &mut self,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [session] = args else {
            self.session
                .report(type_error(span, "`targetTriple` takes a session"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        Some(self.read_source_buffer(
            "klfsvc_session_target_len",
            "klfsvc_session_target_copy",
            vec![session],
        ))
    }

    /// The driver's working directory, through
    /// `klf.process.current_directory`: asked once for its length with no
    /// room, then copied. It needs no session, so `compatDriver` can read it.
    pub(super) fn lower_driver_directory(
        &mut self,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        if !args.is_empty() {
            self.session
                .report(type_error(span, "`driverDirectory` takes no arguments"));
            return None;
        }
        let service = Name::new("klfsvc_process_current_directory");
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let none = self.fb.heap_alloc_array(Type::Integer(8), zero.clone());
        let empty = self.fb.convert(ConvertKind::Bitcast, none, Type::Pointer);
        let len = self.fb.call_foreign(service.clone(), vec![empty, zero.clone()]);
        let known = self
            .fb
            .compare(IntegerPredicate::SignedGe, len.clone(), zero);
        self.trap_unless(
            known,
            "the current directory could not be read",
            "the directory the driver runs in was removed or is not readable; run it from \
             an existing directory",
        );
        let buffer = self.fb.heap_alloc_array(Type::Integer(8), len.clone());
        let data = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        let copied = self
            .fb
            .call_foreign(service, vec![data.clone(), len.clone()]);
        let whole = self
            .fb
            .compare(IntegerPredicate::Eq, copied, len.clone());
        self.trap_unless(
            whole,
            "the current directory changed while it was read",
            "keep the working directory fixed while the driver runs",
        );
        Some((self.build_string(data, len), string_ty()))
    }

    /// A text an out read (`KLF-KSL.md` section 6.7), through
    /// `klf.shader.read.text_len` and `klf.shader.read.text_copy`.
    pub(super) fn lower_shader_text(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [unit, id] = args else {
            self.session
                .report(type_error(span, "`shaderText` takes a unit and a text id"));
            return None;
        };
        let (unit, _) = self.lower_expr(&unit.value)?;
        let (id, _) = self.lower_expr(&id.value)?;
        Some(self.read_source_buffer(
            "klfsvc_shader_read_text_len",
            "klfsvc_shader_read_text_copy",
            vec![unit, id],
        ))
    }

    pub(super) fn lower_source_name(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if !args.is_empty() {
            self.session
                .report(type_error(span, "`sourceName` takes no arguments"));
            return None;
        }
        Some(self.read_source_buffer(
            "klfsvc_source_name_len",
            "klfsvc_source_name_copy",
            Vec::new(),
        ))
    }

    pub(super) fn lower_include_text(
        &mut self,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [path_arg] = args else {
            self.session
                .report(type_error(span, "`includeText` takes one String"));
            return None;
        };
        let (path, ty) = self.lower_expr(&path_arg.value)?;
        if ty != string_ty() {
            self.session.report(type_error(
                path_arg.value.span,
                "`includeText` expects a String",
            ));
            return None;
        }
        let bytes = self.string_bytes(&path);
        let pointer = self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer);
        let length = self.string_len(&path);
        Some(self.read_source_buffer(
            "klfsvc_source_named_len",
            "klfsvc_source_named_copy",
            vec![pointer, length],
        ))
    }

    pub(super) fn lower_include_exists(
        &mut self,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        let [path_arg] = args else {
            self.session
                .report(type_error(span, "`includeExists` takes one String"));
            return None;
        };
        let (path, ty) = self.lower_expr(&path_arg.value)?;
        if ty != string_ty() {
            self.session.report(type_error(
                path_arg.value.span,
                "`includeExists` expects a String",
            ));
            return None;
        }
        let bytes = self.string_bytes(&path);
        let pointer = self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer);
        let length = self.string_len(&path);
        let found = self.fb.call_foreign(
            Name::new("klfsvc_source_named_exists"),
            vec![pointer, length],
        );
        let one = self.fb.const_int(Type::Integer(64), 1);
        Some((
            self.fb.compare(IntegerPredicate::Eq, found, one),
            FeType::Bool,
        ))
    }

    /// Reads a host-side byte buffer into a `String`, one service call per
    /// byte. `length` and `byte` name the two services and `prefix` holds the
    /// arguments they both take before the index.
    pub(super) fn read_source_buffer(
        &mut self,
        length: &str,
        copy: &str,
        prefix: Vec<Name>,
    ) -> (Name, FeType) {
        let len = self.fb.call_foreign(Name::new(length), prefix.clone());
        let zero = self.fb.const_int(Type::Integer(64), 0);
        let known = self
            .fb
            .compare(IntegerPredicate::SignedGe, len.clone(), zero);
        self.trap_unless(
            known,
            &format!("kernel service `{length}` could not measure the text"),
            "the source or name it reads is not loaded in this session",
        );
        let buffer = self.fb.heap_alloc_array(Type::Integer(8), len.clone());
        let data = self.fb.convert(ConvertKind::Bitcast, buffer, Type::Pointer);
        let mut arguments = prefix;
        arguments.extend([data.clone(), len.clone()]);
        let copied = self.fb.call_foreign(Name::new(copy), arguments);
        let one = self.fb.const_int(Type::Integer(64), 1);
        let valid = self.fb.compare(IntegerPredicate::Eq, copied, one);
        self.trap_unless(
            valid,
            &format!("kernel service `{copy}` could not copy the text"),
            "the text changed while it was read; this is a kernel bug, report it",
        );
        (self.build_string(data, len), string_ty())
    }
}
