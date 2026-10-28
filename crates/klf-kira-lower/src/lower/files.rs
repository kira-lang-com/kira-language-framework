//! `readFile`, `readDirectory`, `readHeader` and `readRidl`: a phase reads
//! source files it was not given (section 8.1 `Session.read`), such as a
//! package manifest, the sources of an imported library, the C header of a
//! native one or the contract of a framework.
//!
//! `readFile(session, path)` is the file's id, or -1 when it cannot be read.
//! `readHeader(session, origin, path, library, selection, includes)` is a new file
//! of the `.klffi` declarations `klf ffi gen` makes of the header at `path`, with
//! `includes` the include directories, one per line.
//! `readRidl(session, origin, directory, module)` is a new file of the Kira
//! declarations `klf ridl bindgen --lang kira` makes of the `.ridl` files under
//! `directory`, the contract of `module` (KLF-RIDL section 27.7).
//! `driverPathExists(path)` is whether anything is at `path`, asked of the
//! host rather than of a session, so `compatDriver`, which runs before there
//! is one, can find a project around the working directory.
//! `readDirectory(session, directory, extension)` reads every file under
//! `directory` with that extension and is how many it read, or -1; the prelude's
//! `source_directory_file(session, index)` names each one. A file id is an
//! `Int`, and `as SourceFile` turns it into the handle `fileText`, `fileName`
//! and `diag_error_in` take.

use super::{Builtin, Lowerer};
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_kir::{ConvertKind, Type};
use klf_kira_syntax::ast::Arg;
use klf_source::Span;

impl Lowerer<'_> {
    pub(crate) fn try_file_builtin(&mut self, name: &str, args: &[Arg], span: Span) -> Builtin {
        let result = match name {
            "addSource" => self.lower_add_source(args, span),
            "readHeader" => self.lower_read_header(args, span),
            "readRidl" => self.lower_read_ridl(args, span),
            "readFile" => self.lower_read_file(args, span),
            "pathExists" => self.lower_path_exists(args, span),
            "driverPathExists" => self.lower_driver_path_exists(args, span),
            "readDirectory" => self.lower_read_directory(args, span),
            _ => return Builtin::Skipped,
        };
        Builtin::from_lowered(result)
    }

    fn lower_add_source(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, origin, text] = args else {
            self.session.report(type_error(
                span,
                "`addSource` takes a session, origin file and source text",
            ));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let (origin, _) = self.lower_expr(&origin.value)?;
        let mut arguments = vec![session, origin];
        arguments.extend(self.string_argument(text, "addSource")?);
        Some((
            self.fb
                .call_foreign(Name::new("klfsvc_source_virtual"), arguments),
            int64(),
        ))
    }

    /// `readHeader(session, origin, path, library, selection, includes, scalarAliases)`:
    /// the `.klffi` declarations of a C header as a new source file beside `origin`,
    /// or -1. `scalarAliases` is "true" or "false" and says whether the scalar
    /// pointer declarations (`float_ptr` and the like) are written.
    fn lower_read_header(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, origin, path, library, selection, includes, scalar_aliases] = args else {
            self.session.report(type_error(
                span,
                "`readHeader` takes a session, origin file, header path, library, selection, include directories and scalar pointer aliases",
            ));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let (origin, _) = self.lower_expr(&origin.value)?;
        let mut arguments = vec![session, origin];
        arguments.extend(self.string_argument(path, "readHeader")?);
        arguments.extend(self.string_argument(library, "readHeader")?);
        arguments.extend(self.string_argument(selection, "readHeader")?);
        arguments.extend(self.string_argument(includes, "readHeader")?);
        arguments.extend(self.string_argument(scalar_aliases, "readHeader")?);
        Some((
            self.fb
                .call_foreign(Name::new("klfsvc_source_ffi_header"), arguments),
            int64(),
        ))
    }

    /// `readRidl(session, origin, directory, module)`: the Kira declarations of
    /// the contract under `directory` as a new source file beside `origin`, or
    /// -1.
    fn lower_read_ridl(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, origin, directory, module] = args else {
            self.session.report(type_error(
                span,
                "`readRidl` takes a session, origin file, contract directory and module",
            ));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let (origin, _) = self.lower_expr(&origin.value)?;
        let mut arguments = vec![session, origin];
        arguments.extend(self.string_argument(directory, "readRidl")?);
        arguments.extend(self.string_argument(module, "readRidl")?);
        Some((
            self.fb
                .call_foreign(Name::new("klfsvc_source_ridl_module"), arguments),
            int64(),
        ))
    }

    fn lower_read_file(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, path] = args else {
            self.session
                .report(type_error(span, "`readFile` takes a session and a path"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let mut arguments = vec![session];
        arguments.extend(self.string_argument(path, "readFile")?);
        let file = self
            .fb
            .call_foreign(Name::new("klfsvc_source_read"), arguments);
        Some((file, int64()))
    }

    /// Whether a file or directory is at `path`, which a definition asks
    /// before naming a file it does not read, such as a library to link.
    fn lower_path_exists(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, path] = args else {
            self.session
                .report(type_error(span, "`pathExists` takes a session and a path"));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let mut arguments = vec![session];
        arguments.extend(self.string_argument(path, "pathExists")?);
        let found = self
            .fb
            .call_foreign(Name::new("klfsvc_source_path_exists"), arguments);
        let one = self.fb.const_int(Type::Integer(64), 1);
        Some((
            self.fb.compare(klf_kir::IntegerPredicate::Eq, found, one),
            FeType::Bool,
        ))
    }

    /// Whether anything is at `path`, through `klf.fs.path_exists`.
    fn lower_driver_path_exists(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [path] = args else {
            self.session
                .report(type_error(span, "`driverPathExists` takes a path"));
            return None;
        };
        let arguments = self.string_argument(path, "driverPathExists")?;
        let found = self
            .fb
            .call_foreign(Name::new("klfsvc_fs_path_exists"), arguments.to_vec());
        let one = self.fb.const_int(Type::Integer(64), 1);
        Some((
            self.fb.compare(klf_kir::IntegerPredicate::Eq, found, one),
            FeType::Bool,
        ))
    }

    fn lower_read_directory(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        let [session, directory, extension] = args else {
            self.session.report(type_error(
                span,
                "`readDirectory` takes a session, a directory and an extension",
            ));
            return None;
        };
        let (session, _) = self.lower_expr(&session.value)?;
        let mut arguments = vec![session];
        arguments.extend(self.string_argument(directory, "readDirectory")?);
        arguments.extend(self.string_argument(extension, "readDirectory")?);
        let count = self
            .fb
            .call_foreign(Name::new("klfsvc_source_directory"), arguments);
        Some((count, int64()))
    }

    /// A service call's arguments with each `String` spread into the
    /// (pointer, length) pair the service reads.
    pub(super) fn text_arguments(
        &mut self,
        signature: &crate::Signature,
        values: Vec<Name>,
    ) -> Vec<Name> {
        let text = FeType::Struct(Name::new("String"));
        let mut spread = Vec::with_capacity(values.len());
        for (value, ty) in values.into_iter().zip(signature.arguments()) {
            if *ty != text {
                spread.push(value);
                continue;
            }
            let bytes = self.string_bytes(&value);
            spread.push(self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer));
            spread.push(self.string_len(&value));
        }
        spread
    }

    /// A `String` argument as the (pointer, length) pair a service reads.
    pub(super) fn string_argument(&mut self, argument: &Arg, builtin: &str) -> Option<[Name; 2]> {
        let (text, ty) = self.lower_expr(&argument.value)?;
        if ty != FeType::Struct(Name::new("String")) {
            self.session.report(type_error(
                argument.value.span,
                format!("`{builtin}` expects a String here"),
            ));
            return None;
        }
        let bytes = self.string_bytes(&text);
        let pointer = self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer);
        Some([pointer, self.string_len(&text)])
    }
}

fn int64() -> FeType {
    FeType::Int {
        bits: 64,
        signed: true,
    }
}
