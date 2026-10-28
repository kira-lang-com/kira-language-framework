//! The kernel entry around a `Language.project` hook (`KLF-EDITOR.md`
//! section 5.3).
//!
//! The hook takes a `String` path and returns `Optional<Project>`, neither of
//! which crosses the word-sized phase ABI. The lowerer therefore emits
//! `__klf_project(session, count) -> Int`, which rebuilds the path from the
//! `klf.driver.argument_*` services (it is argument 0), calls the hook, and
//! hands a `Some` result back one string at a time through
//! `klf.editor.project_field`. It returns 0 for `None` and 1 for `Some`.

use super::Lowerer;
use super::compat_driver::entry_lowerer;
use crate::ty::FeType;
use klf_core::{FileId, Name};
use klf_kir::{BlockTarget, ConvertKind, Function, IntegerPredicate, Linkage, Type};
use klf_kira_syntax::ast::{DeclKind, File};
use klf_services::{FunctionBuilder, Session};
use klf_source::{SourceFile, Span};

pub(crate) const PROJECT: &str = "project";
const ENTRY: &str = "__klf_project";
const STRUCT: &str = "Project";

/// The kernel's `Project` (section 5.2), declared for a package whose
/// language has a `project` hook and does not declare its own.
const PRELUDE: &str = "\
struct Project {
    let name: String
    let root: String
    let arguments: [String]
    let watch: [String]
    let output: Optional<String> = .None
}
";

/// What each string the entry hands back is, as `klf.editor.project_field`
/// numbers it.
const NAME: i64 = 0;
const ROOT: i64 = 1;
const ARGUMENT: i64 = 2;
const WATCH: i64 = 3;
const OUTPUT: i64 = 4;

/// Adds the kernel's `Project` to a package that writes a `project` hook.
pub(crate) fn prepare(mut file: File) -> File {
    let declared = file.decls.iter().any(|decl| match &decl.kind {
        DeclKind::Struct(declared) => declared.name.name.as_str() == STRUCT,
        DeclKind::Enum(declared) => declared.name.name.as_str() == STRUCT,
        _ => false,
    });
    if declared || !crate::definition_entrypoints::has_hook(&file, PROJECT) {
        return file;
    }
    let prelude = SourceFile::new(FileId::from_u32(0), "<klf-project>", PRELUDE);
    file.decls
        .extend(klf_kira_syntax::parse(&prelude).file.decls);
    file
}

pub(crate) fn lower_entry(
    session: &mut Session,
    elaborated: &crate::ElaboratedFile,
    counts: &std::cell::RefCell<super::ownership::NeededFunctions>,
    span: Span,
) -> Function {
    let handle = Name::new("session");
    let count = Name::new("count");
    let mut fb = FunctionBuilder::new(
        Name::new(ENTRY),
        &[
            (handle.clone(), Type::Integer(64)),
            (count, Type::Integer(64)),
        ],
        Type::Integer(64),
        span,
    );
    fb.set_linkage(Linkage::Module);
    let mut lowerer = entry_lowerer(session, elaborated, counts, fb);
    let first = lowerer.fb.const_int(Type::Integer(64), 0);
    let path = lowerer.copy_argument(first);

    let project = FeType::Struct(Name::new(STRUCT));
    let optional = crate::generics::instance_name("Optional", &[project]);
    let result = lowerer.fb.stack_alloc(Type::Nominal(optional, Vec::new()));
    lowerer
        .fb
        .call_void(Name::new(PROJECT), vec![result.clone(), handle, path]);
    let project_type = Type::Nominal(Name::new(STRUCT), Vec::new());
    let some = lowerer.some_case(result, project_type, None);
    lowerer.push_field(&some, "name", NAME);
    lowerer.push_field(&some, "root", ROOT);
    lowerer.push_list_field(&some, "arguments", ARGUMENT);
    lowerer.push_list_field(&some, "watch", WATCH);
    let output = lowerer.project_field(&some, "output");
    let string = Type::Nominal(Name::new("String"), Vec::new());
    let done = lowerer.fb.new_block(&[]);
    let value = lowerer.some_case(output, string, Some(&done));
    lowerer.push_project_string(&value, OUTPUT);
    lowerer.fb.branch(done.clone(), Vec::new());
    lowerer.fb.switch_to(&done);
    let one = lowerer.fb.const_int(Type::Integer(64), 1);
    lowerer.fb.ret(Some(one));

    let mut function = lowerer.fb.finish();
    function.abi = klf_kir::FunctionAbi::Ridl;
    function
}

impl Lowerer<'_> {
    /// Switches on the `Optional` at `address` and continues in its `Some`
    /// path, answering the payload's address. `None` goes to `absent`, or
    /// returns 0 from the entry when there is none.
    fn some_case(&mut self, address: Name, payload: Type, absent: Option<&Name>) -> Name {
        let none = self.fb.new_block(&[]);
        let value = self.fb.fresh_value();
        let present = self
            .fb
            .new_block(&[(value.clone(), Type::Address(Box::new(payload)))]);
        let payload_address = self.fb.enum_payload_addr(address.clone(), 1);
        self.fb.switch_enum(
            address,
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
                        block: present.clone(),
                        arguments: vec![payload_address],
                    },
                ),
            ],
            None,
        );
        self.fb.switch_to(&none);
        match absent {
            Some(block) => self.fb.branch(block.clone(), Vec::new()),
            None => {
                let zero = self.fb.const_int(Type::Integer(64), 0);
                self.fb.ret(Some(zero));
            }
        }
        self.fb.switch_to(&present);
        value
    }

    fn project_field(&mut self, project: &Name, field: &str) -> Name {
        let index = self
            .structs
            .get(&Name::new(STRUCT))
            .and_then(|info| info.field(field))
            .map_or(0, |(index, _)| index);
        self.fb.field_addr(project.clone(), index)
    }

    fn push_field(&mut self, project: &Name, field: &str, kind: i64) {
        let string = self.project_field(project, field);
        self.push_project_string(&string, kind);
    }

    fn push_list_field(&mut self, project: &Name, field: &str, kind: i64) {
        let list = self.project_field(project, field);
        let count = self.list_count(&list);
        let string = FeType::Struct(Name::new("String"));
        self.counted_loop(&count, |lowerer, index| {
            let element = lowerer.list_element_addr(&list, &string, index);
            lowerer.push_project_string(&element, kind);
        });
    }

    fn push_project_string(&mut self, string: &Name, kind: i64) {
        let bytes = self.string_bytes(string);
        let pointer = self.fb.convert(ConvertKind::Bitcast, bytes, Type::Pointer);
        let length = self.string_len(string);
        let kind = self.fb.const_int(Type::Integer(64), i128::from(kind));
        let pushed = self.fb.call_foreign(
            Name::new("klfsvc_editor_project_field"),
            vec![kind, pointer, length],
        );
        let one = self.fb.const_int(Type::Integer(64), 1);
        let valid = self.fb.compare(IntegerPredicate::Eq, pushed, one);
        self.trap_unless(
            valid,
            "the editor refused a project field the definition returned",
            "a project field is UTF-8 text; check the value for invalid bytes",
        );
    }
}
