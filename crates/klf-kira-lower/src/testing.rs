//! The `Testing` module (`KLF-TESTING.md`).
//!
//! A definition package's `tests/*.klfd` files `import Testing` and mark test
//! functions with `@Test`. They compile into the package's own image, so a
//! test can call the definition's helpers directly, and `klf test` calls each
//! one on the VM. `expect!(condition)` records an issue with the condition's
//! source text and position when it is false; the test goes on. The
//! functions below give a test the language under test: `run` compiles a
//! program with it and runs its `main` (`runPath` reads it from a file of the
//! package), `compiles` only compiles, `formats` checks the format hook, and
//! `diagnosed` and `reported` look at what the last compilation reported.

use klf_core::{FileId, Name};
use klf_diag::Diagnostic;
use klf_kira_syntax::ast::{Decl, DeclKind, File, FunctionDecl};
use klf_services::Session;
use klf_source::SourceFile;

/// The module a test file imports.
pub const MODULE: &str = "Testing";

/// The annotation that makes a function a test.
const TEST: &str = "Test";

const PRELUDE: &str = "\
struct TestSource {
    let name: String
    let text: String
}

function run(source: String) -> Int {
    test_source(\"\", source)
    return test_run()
}

function runFiles(sources: [TestSource]) -> Int {
    for source in sources {
        test_source(source.name, source.text)
    }
    return test_run()
}

function runPath(path: String) -> Int {
    test_source_path(path)
    return test_run()
}

function runPaths(paths: [String]) -> Int {
    for path in paths {
        test_source_path(path)
    }
    return test_run()
}

function compiles(source: String) -> Bool {
    test_source(\"\", source)
    return test_compile() == 1
}

function compilesFiles(sources: [TestSource]) -> Bool {
    for source in sources {
        test_source(source.name, source.text)
    }
    return test_compile() == 1
}

function formats(source: String, expected: String) -> Bool {
    return test_formats(source, expected) == 1
}

function diagnosed(code: Int, line: Int, column: Int) -> Bool {
    return test_diagnosed(code, line, column) == 1
}

function reported(text: String) -> Bool {
    return test_reported(text) == 1
}
";

pub(crate) fn imports_testing(file: &File) -> bool {
    file.imports.iter().any(|import| {
        import.path.segments.len() == 1 && import.path.segments[0].name.as_str() == MODULE
    })
}

fn is_test(decl: &Decl) -> bool {
    matches!(decl.kind, DeclKind::Function(_))
        && decl.annotations.iter().any(|annotation| {
            annotation.path.segments.len() == 1 && annotation.path.segments[0].name.as_str() == TEST
        })
}

/// The names of the `@Test` functions of `file`, in declaration order.
#[must_use]
pub fn tests(file: &File) -> Vec<String> {
    file.decls
        .iter()
        .filter(|decl| is_test(decl))
        .filter_map(|decl| match &decl.kind {
            DeclKind::Function(function) => Some(function.name.name.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

/// Checks every `@Test` function, takes the annotation off, and gives a file
/// that imports `Testing` the module's declarations.
pub(crate) fn prepare(session: &mut Session, mut file: File) -> File {
    let testing = imports_testing(&file);
    for decl in &mut file.decls {
        if !is_test(decl) {
            continue;
        }
        let DeclKind::Function(function) = &decl.kind else {
            continue;
        };
        if !testing {
            report(session, function, "`@Test` needs `import Testing`");
        } else if !function.params.is_empty() || function.ret.is_some() {
            report(
                session,
                function,
                "a test takes no parameters and returns nothing",
            );
        }
        decl.annotations.retain(|annotation| {
            !(annotation.path.segments.len() == 1
                && annotation.path.segments[0].name.as_str() == TEST)
        });
    }
    if testing {
        let prelude = SourceFile::new(FileId::from_u32(0), "<klf-testing>", PRELUDE);
        file.decls
            .extend(klf_kira_syntax::parse(&prelude).file.decls);
    }
    file
}

fn report(session: &mut Session, function: &FunctionDecl, message: &str) {
    session.report(
        Diagnostic::error_at("KLFK-E0207", function.name.span, message.to_owned())
            .with_label(function.name.span, "this test"),
    );
}

/// Whether `name` is the `expect!` macro.
pub(crate) fn is_expect(name: &Name) -> bool {
    name.as_str() == "expect!"
}
