//! The compile-and-run pipeline behind the `klf` commands.

use klf_driver::{
    Backend, compile_package as driver_compile_package, run as driver_run,
    run_with_source_into_session as driver_run_with_source_into_session,
};
use klf_kir::{Module, parse_module};
use klf_kira_lower::KiraDefinition;
use klf_services::{Definition, DefinitionPackage, DefinitionRegistry, Session};
use klf_target::Target;
use klf_toy_calc::CalcDefinition;
use klf_vm::Outcome;

/// A rendered compile failure: the human-readable diagnostics collected so far.
#[derive(Debug)]
pub struct CompileError {
    pub rendered: String,
}

pub struct CompiledProgram {
    pub module: Module,
    pub session: Session,
}

/// Picks the language definition for a file by its extension. Unknown
/// extensions default to KLF Kira.
fn definition_registry() -> DefinitionRegistry {
    let mut registry = DefinitionRegistry::new();
    registry
        .register(CalcDefinition)
        .expect("calc metadata is unique");
    registry
        .register(KiraDefinition)
        .expect("KLF Kira metadata is unique");
    registry
}

fn definition_for<'a>(registry: &'a DefinitionRegistry, file_name: &str) -> &'a dyn Definition {
    registry
        .select(file_name)
        .or_else(|| registry.by_name("klfkira"))
        .expect("the built-in KLF Kira definition is registered")
}

/// Parses and lowers source to a KIR module, or returns rendered diagnostics.
/// The definition is chosen from the file's extension.
pub fn compile(
    source: &str,
    file_name: &str,
    module_name: &str,
    target: Target,
) -> Result<Module, CompileError> {
    compile_with_session(source, file_name, module_name, target).map(|program| program.module)
}

pub fn compile_with_session(
    source: &str,
    file_name: &str,
    module_name: &str,
    target: Target,
) -> Result<CompiledProgram, CompileError> {
    let registry = definition_registry();
    let definition = definition_for(&registry, file_name);
    let mut session = Session::new(target.clone());
    let language = definition.language();
    if !session.register_diagnostics(language.diagnostics) {
        return Err(CompileError {
            rendered: format!("{file_name}: duplicate diagnostic specification\n"),
        });
    }
    let package = DefinitionPackage::single(file_name, source);
    let module = driver_compile_package(definition, &mut session, &package, module_name, target);
    match module {
        Some(module) if session.error_count() == 0 => Ok(CompiledProgram { module, session }),
        _ => Err(CompileError {
            rendered: render(&session),
        }),
    }
}

/// Parses a textual KIR module. Used by `klf run`/`klf build` on a `.kir` file
/// so already-emitted KIR (for example a frontend's output) can be executed or
/// re-checked without going through a language definition.
///
/// # Errors
/// Returns a rendered parse error when the text is not a valid KIR module.
pub fn parse_kir(source: &str, file_name: &str) -> Result<Module, String> {
    parse_module(source).map_err(|error| format!("{file_name}: {error:?}\n"))
}

/// Runs one function of a module with integer arguments on a backend.
///
/// # Errors
/// Returns a message when the module cannot be run or the entry is missing.
pub fn run(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    backend: Backend,
) -> Result<Outcome, String> {
    driver_run(module, entry, arguments, backend)
}

pub fn run_compiled_with_source(
    program: &mut CompiledProgram,
    entry: &str,
    arguments: &[u64],
    source: &str,
    backend: Backend,
) -> Result<Outcome, String> {
    let file = program.session.add_file("<runtime>", source);
    driver_run_with_source_into_session(
        &program.module,
        entry,
        arguments,
        source,
        backend,
        &mut program.session,
        file,
    )
}

pub fn render(session: &Session) -> String {
    let mut out = String::new();
    for diagnostic in session.diagnostics() {
        out.push_str(&klf_services::session_diagnostic(session, diagnostic));
    }
    out
}

pub fn render_runtime_diagnostics(session: &Session) -> String {
    render_runtime_diagnostics_from(session, 0)
}

/// The runtime diagnostics the session kept from index `first` on.
pub fn render_runtime_diagnostics_from(session: &Session, first: usize) -> String {
    let mut out = String::new();
    for diagnostic in &session.runtime_diagnostics()[first..] {
        out.push_str(&klf_services::runtime_diagnostic(session, diagnostic));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_driver::{
        run_with_source as driver_run_with_source, run_with_source_and_diagnostics,
        toolchain_available,
    };

    const SERVICES: &str = r#"module @Services lang "klfkira" target "aarch64-apple-darwin" stage canonical

func private foreign @klfsvc_io_put(%value: i32) -> i32 engine(native)
func private foreign @klfsvc_diag_error(%code: i64, %start: i64, %end: i64) -> unit engine(native)

func module @main() -> i64 engine(native) {
entry():
  %value = const.int i32, 65
  %written = call.foreign @klfsvc_io_put(%value)
  %code = const.int i64, 2001
  %start = const.int i64, 0
  %end = const.int i64, 1
  call.foreign @klfsvc_diag_error(%code, %start, %end)
  %result = const.int i64, 42
  return %result
}
"#;

    fn target() -> Target {
        "aarch64-apple-darwin".parse().expect("valid target")
    }

    #[test]
    fn compiles_and_runs_klfkira() {
        let module = compile(
            "function main() -> Int { return 6 * 7 }",
            "t.klfk",
            "Main",
            target(),
        )
        .expect("compiles");
        assert_eq!(
            run(&module, "main", &[], Backend::Vm).expect("runs"),
            Outcome::Return(Some(42))
        );
    }

    #[test]
    fn runs_definition_diagnostics_through_the_session() {
        let mut program = compile_with_session(
            "function main() -> Int { diag_error(2001, 0, 1) return 42 }",
            "t.klfk",
            "Diagnostics",
            target(),
        )
        .expect("compiles");
        assert_eq!(
            run_compiled_with_source(&mut program, "main", &[], "x", Backend::Vm).expect("runs"),
            Outcome::Return(Some(42))
        );
        assert_eq!(
            render_runtime_diagnostics(&program.session),
            "<runtime>:1:1: error[2001]: runtime diagnostic: a definition reported an error\n"
        );
    }

    #[test]
    fn executes_kira_definition_corpus_on_vm_and_native() {
        const DEFINITION: &str = include_str!("../../../definitions/calc.klfd");
        const INPUT: &str = "20 + 2 * (10 + 1)";
        let module =
            compile(DEFINITION, "calc.klfd", "CalcDefinition", target()).expect("compiles");
        assert_eq!(
            driver_run_with_source(&module, "main", &[], INPUT, Backend::Vm).expect("VM runs"),
            Outcome::Return(Some(42))
        );
        if toolchain_available() {
            assert_eq!(
                driver_run_with_source(&module, "main", &[], INPUT, Backend::Native)
                    .expect("native runs"),
                Outcome::Return(Some(42))
            );
        }
    }

    #[test]
    fn links_manifest_service_symbols_natively() {
        if !toolchain_available() {
            return;
        }
        let module = klf_kir::parse_module(SERVICES).expect("services module parses");
        assert_eq!(
            run_with_source_and_diagnostics(&module, "main", &[], "A", Backend::Native)
                .expect("native runs"),
            (
                Outcome::Return(Some(42)),
                vec![klf_diag::RuntimeDiagnostic::new(2001, 0, 1)],
            )
        );
    }

    #[test]
    fn selects_definitions_through_language_metadata() {
        let registry = definition_registry();
        assert_eq!(registry.len(), 2);
        assert_eq!(
            definition_for(&registry, "input.calc").language().name,
            "calc"
        );
        assert_eq!(
            definition_for(&registry, "definition.klfd").language().name,
            "klfkira"
        );
        assert_eq!(
            definition_for(&registry, "definition.unknown")
                .language()
                .name,
            "klfkira"
        );
    }

    #[test]
    fn reports_type_error_with_location() {
        let error = compile(
            "function main() -> Int { return true }",
            "t.klfk",
            "Main",
            target(),
        )
        .expect_err("should fail");
        assert!(error.rendered.contains("t.klfk:1:"), "{}", error.rendered);
        assert!(error.rendered.contains("KLFK-E0205"), "{}", error.rendered);
    }
}
