//! The compile-and-run pipeline behind the `klf` commands.

use klf_driver::{
    Backend, compile_package_shared as driver_compile_package_shared, run as driver_run,
    run_with_source_into_session as driver_run_with_source_into_session,
};
use klf_kir::{Module, parse_module};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionGraph, DefinitionPackage, DefinitionRegistry, GraphError, PackageId,
    Session,
};
use klf_target::Target;
use klf_toy_calc::CalcDefinition;
use klf_vm::Outcome;
use std::collections::HashSet;
use std::sync::Arc;

/// A rendered compile failure: the human-readable diagnostics collected so far.
#[derive(Debug)]
pub struct CompileError {
    pub rendered: String,
}

pub struct CompiledProgram {
    pub module: Arc<Module>,
    pub session: Session,
}

/// A reusable in-process compiler and query session for editor or build
/// service requests. Keeping this value alive reuses source IDs and valid
/// parse, elaborate, and lower results across requests.
pub struct CompilerSession {
    registry: DefinitionRegistry,
    session: Session,
    graph: DefinitionGraph,
    registered_diagnostics: HashSet<String>,
}

impl CompilerSession {
    #[must_use]
    pub fn new(target: Target) -> Self {
        Self {
            registry: definition_registry(),
            session: Session::new(target),
            graph: DefinitionGraph::new(),
            registered_diagnostics: HashSet::new(),
        }
    }

    /// Adds one logical package to this compiler session's unified source
    /// graph. Package boundaries remain metadata for dependency validation;
    /// all source files are parsed, checked, and lowered as one unit.
    pub fn add_package(
        &mut self,
        name: impl Into<String>,
        module_root: impl Into<String>,
        dependencies: Vec<String>,
        package: &DefinitionPackage,
    ) -> Result<PackageId, GraphError> {
        self.graph
            .add_package(name, module_root, dependencies, package)
    }

    /// Replaces one graph source while preserving its file identity and
    /// incremental query history.
    pub fn replace_package_source(
        &mut self,
        package: &str,
        source: &str,
        text: impl Into<Arc<str>>,
    ) -> Result<bool, GraphError> {
        self.graph.replace_source(package, source, text)
    }

    #[must_use]
    pub fn package_count(&self) -> usize {
        self.graph.packages().len()
    }

    /// Checks every source in the unified package graph and leaves parse and
    /// elaboration results cached for a subsequent build.
    pub fn check_graph(&mut self) -> Result<(), CompileError> {
        self.session.clear_diagnostics();
        let definition = self
            .registry
            .by_name("klfkira")
            .expect("the built-in KLF Kira definition is registered");
        Self::register_definition_diagnostics(
            &mut self.session,
            &mut self.registered_diagnostics,
            definition,
        )?;
        self.graph
            .resolve_dependencies()
            .map_err(|error| CompileError {
                rendered: format!("package dependency resolution failed: {error}\n"),
            })?;
        let Some(unit) = self.graph.register_sources(&mut self.session) else {
            return Err(CompileError {
                rendered: "package graph contains no source files\n".to_owned(),
            });
        };
        let Some(parsed) = definition.parse(&mut self.session, unit.clone()) else {
            return Err(CompileError {
                rendered: render(&self.session),
            });
        };
        if definition
            .elaborate(&mut self.session, unit, &parsed)
            .is_none()
            || self.session.error_count() != 0
        {
            return Err(CompileError {
                rendered: render(&self.session),
            });
        }
        Ok(())
    }

    /// Builds the package graph through the same persistent parse, check, and
    /// lowering queries used by `check_graph` and editor requests.
    pub fn compile_graph_shared(&mut self, module_name: &str) -> Result<Arc<Module>, CompileError> {
        self.session.clear_diagnostics();
        let definition = self
            .registry
            .by_name("klfkira")
            .expect("the built-in KLF Kira definition is registered");
        Self::register_definition_diagnostics(
            &mut self.session,
            &mut self.registered_diagnostics,
            definition,
        )?;
        self.graph
            .resolve_dependencies()
            .map_err(|error| CompileError {
                rendered: format!("package dependency resolution failed: {error}\n"),
            })?;
        let Some(unit) = self.graph.register_sources(&mut self.session) else {
            return Err(CompileError {
                rendered: "package graph contains no source files\n".to_owned(),
            });
        };
        let Some(parsed) = definition.parse(&mut self.session, unit.clone()) else {
            return Err(CompileError {
                rendered: render(&self.session),
            });
        };
        let Some(elaborated) = definition.elaborate(&mut self.session, unit.clone(), &parsed)
        else {
            return Err(CompileError {
                rendered: render(&self.session),
            });
        };
        let target = self.session.target().clone();
        let Some(module) =
            definition.lower_shared(&mut self.session, unit, &elaborated, module_name, target)
        else {
            return Err(CompileError {
                rendered: render(&self.session),
            });
        };
        if self.session.error_count() == 0 {
            Ok(module)
        } else {
            Err(CompileError {
                rendered: render(&self.session),
            })
        }
    }

    /// Compiles one source through the shared KLF frontend and returns its KIR.
    /// Repeated identical requests reuse every valid phase result.
    pub fn compile_source(
        &mut self,
        source: &str,
        file_name: &str,
        module_name: &str,
    ) -> Result<Module, CompileError> {
        self.compile_source_shared(source, file_name, module_name)
            .map(Arc::unwrap_or_clone)
    }

    /// Compiles source and returns shared KIR so no-op build services can use
    /// the cached module without copying its functions.
    pub fn compile_source_shared(
        &mut self,
        source: &str,
        file_name: &str,
        module_name: &str,
    ) -> Result<Arc<Module>, CompileError> {
        self.session.clear_diagnostics();
        let definition = definition_for(&self.registry, file_name);
        Self::register_definition_diagnostics(
            &mut self.session,
            &mut self.registered_diagnostics,
            definition,
        )?;
        let package = DefinitionPackage::single(file_name, source);
        let target = self.session.target().clone();
        match driver_compile_package_shared(
            definition,
            &mut self.session,
            &package,
            module_name,
            target,
        ) {
            Some(module) if self.session.error_count() == 0 => Ok(module),
            _ => Err(CompileError {
                rendered: render(&self.session),
            }),
        }
    }

    /// Checks source through parse and elaboration when the language provides
    /// phase queries, leaving its results available to later build requests.
    pub fn check_source(&mut self, source: &str, file_name: &str) -> Result<(), CompileError> {
        self.session.clear_diagnostics();
        let definition = definition_for(&self.registry, file_name);
        Self::register_definition_diagnostics(
            &mut self.session,
            &mut self.registered_diagnostics,
            definition,
        )?;
        if !definition.supports_phase_queries() {
            self.compile_source(source, file_name, "Check")?;
            return Ok(());
        }
        let package = DefinitionPackage::single(file_name, source);
        let units = definition.units(&mut self.session, &package);
        let mut succeeded = units.is_some();
        if let Some(units) = units {
            for unit in units {
                let Some(parsed) = definition.parse(&mut self.session, unit.clone()) else {
                    succeeded = false;
                    break;
                };
                if definition
                    .elaborate(&mut self.session, unit, &parsed)
                    .is_none()
                {
                    succeeded = false;
                    break;
                }
            }
        }
        if succeeded && self.session.error_count() == 0 {
            Ok(())
        } else {
            Err(CompileError {
                rendered: render(&self.session),
            })
        }
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn into_session(self) -> Session {
        self.session
    }

    fn register_definition_diagnostics(
        session: &mut Session,
        registered_diagnostics: &mut HashSet<String>,
        definition: &dyn Definition,
    ) -> Result<(), CompileError> {
        let name = definition.name();
        if registered_diagnostics.contains(&name) {
            return Ok(());
        }
        if !session.register_diagnostics(definition.language().diagnostics) {
            return Err(CompileError {
                rendered: format!("{name}: duplicate diagnostic specification\n"),
            });
        }
        registered_diagnostics.insert(name);
        Ok(())
    }
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
    compile_with_session(source, file_name, module_name, target)
        .map(|program| Arc::unwrap_or_clone(program.module))
}

pub fn compile_with_session(
    source: &str,
    file_name: &str,
    module_name: &str,
    target: Target,
) -> Result<CompiledProgram, CompileError> {
    let mut compiler = CompilerSession::new(target);
    let module = compiler.compile_source_shared(source, file_name, module_name)?;
    Ok(CompiledProgram {
        module,
        session: compiler.into_session(),
    })
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
    fn reusable_compiler_shares_check_build_and_noop_queries() {
        let source = "function main() -> Int { return 42 }";
        let mut compiler = CompilerSession::new(target());
        compiler
            .check_source(source, "main.klfk")
            .expect("checks source");
        let after_check = compiler.session().query_stats();
        assert_eq!(after_check.executions, 2);

        let built = compiler
            .compile_source_shared(source, "main.klfk", "Main")
            .expect("build reuses check results");
        let after_build = compiler.session().query_stats();
        assert_eq!(after_build.executions - after_check.executions, 2);
        assert_eq!(after_build.cache_hits - after_check.cache_hits, 2);

        let no_op = compiler
            .compile_source_shared(source, "main.klfk", "Main")
            .expect("no-op build reuses lowered output");
        assert!(Arc::ptr_eq(&built, &no_op));
        let after_noop = compiler.session().query_stats();
        assert_eq!(after_noop.executions, after_build.executions);
        assert_eq!(after_noop.cache_hits - after_build.cache_hits, 3);
    }

    #[test]
    fn reusable_compiler_builds_a_package_graph_as_one_unit() {
        let mut compiler = CompilerSession::new(target());
        compiler
            .add_package(
                "math",
                "Math",
                Vec::new(),
                &DefinitionPackage::single(
                    "math.klfd",
                    "function double(value: Int) -> Int { return value * 2 }",
                ),
            )
            .expect("math package is indexed");
        compiler
            .add_package(
                "app",
                "App",
                vec!["math".to_owned()],
                &DefinitionPackage::single(
                    "main.klfd",
                    "function main() -> Int { return double(21) }",
                ),
            )
            .expect("app package is indexed");
        assert_eq!(compiler.package_count(), 2);

        compiler.check_graph().expect("the shared graph checks");
        let after_check = compiler.session().query_stats();
        assert_eq!(after_check.executions, 4);

        let built = compiler
            .compile_graph_shared("PackageGraph")
            .expect("build reuses unified check results");
        let after_build = compiler.session().query_stats();
        assert_eq!(after_build.executions - after_check.executions, 3);
        assert_eq!(after_build.cache_hits - after_check.cache_hits, 2);
        assert_eq!(
            driver_run(&built, "main", &[], Backend::Vm).expect("cross-package call runs"),
            Outcome::Return(Some(42))
        );

        let no_op = compiler
            .compile_graph_shared("PackageGraph")
            .expect("no-op package graph build reuses lowered output");
        assert!(Arc::ptr_eq(&built, &no_op));
        let after_noop = compiler.session().query_stats();
        assert_eq!(after_noop.executions, after_build.executions);
        assert_eq!(after_noop.cache_hits - after_build.cache_hits, 3);

        assert!(
            compiler
                .replace_package_source(
                    "math",
                    "math.klfd",
                    "function double(value: Int) -> Int { return value * 3 }",
                )
                .expect("math source is indexed")
        );
        let before_edit = compiler.session().query_stats();
        let edited = compiler
            .compile_graph_shared("PackageGraph")
            .expect("one edited function rebuilds");
        let after_edit = compiler.session().query_stats();
        assert_eq!(after_edit.executions - before_edit.executions, 5);
        assert_eq!(after_edit.cache_hits - before_edit.cache_hits, 3);
        assert_eq!(after_edit.invalidated - before_edit.invalidated, 4);
        assert_eq!(
            driver_run(&edited, "main", &[], Backend::Vm).expect("edited call runs"),
            Outcome::Return(Some(63))
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
        const DEFINITION: &str = include_str!("../../../definitions/calc.kcore");
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
