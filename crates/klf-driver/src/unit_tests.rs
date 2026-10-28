//! `klf test`'s unit tests (`KLF-TESTING.md`).
//!
//! `<package>/tests/*.klfd` compile into the package's image beside the
//! definition, and every `@Test` function in them runs on the VM, each on a
//! fresh machine. The `klf.test.*` services a test calls compile programs
//! with the language that image defines, so the package loads once for the
//! whole run.

use crate::corpus::CaseResult;
use crate::phase_runner::run_test;
use crate::{Backend, LanguageDefinition, load_package};
use klf_core::FileId;
use klf_defsvc::ServiceSink;
use klf_rt::ServiceMemory;
use klf_services::{DefinitionPackage, DefinitionSource, Session};
use klf_source::SourceFile;
use klf_target::Target;
use klf_vm::Outcome;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What one test has done so far.
#[derive(Default)]
struct TestState {
    issues: Vec<String>,
    sources: Vec<(String, String)>,
    diagnostics: Vec<String>,
    places: Vec<(i64, i64, i64)>,
}

/// Runs every `@Test` function under `<package>/tests`; no tests is no
/// results.
///
/// # Errors
/// Returns a message when a test file cannot be read or the package does not
/// compile with its tests.
pub fn run_unit_tests(session: &mut Session, package: &Path) -> Result<Vec<CaseResult>, String> {
    let tests = test_sources(&package.join("tests"))?;
    if tests.is_empty() {
        return Ok(Vec::new());
    }
    let loaded = load_package(package).map_err(|error| error.to_string())?;
    let mut sources = loaded.sources().to_vec();
    sources.extend(tests.iter().cloned());
    let combined = DefinitionPackage::from_sources(sources);
    let language = LanguageDefinition::from_package(session, &combined, Backend::Vm)
        .ok_or_else(|| format!("{} does not compile with its tests", package.display()))?;
    let language = Arc::new(language);
    let program = language.phase_program()?;
    let target = session.target().clone();
    let mut results = Vec::new();
    for test in &tests {
        let file = SourceFile::new(FileId::from_u32(0), test.name.as_str(), test.text.clone());
        for name in klf_kira_lower::testing::tests(&klf_kira_syntax::parse(&file).file) {
            let state = Arc::new(Mutex::new(TestState::default()));
            let mut sink = ServiceSink::new();
            register(&mut sink, &state, &language, &target, package);
            let outcome = run_test(program, &name, sink);
            let mut state = state.lock().map_err(|_| "test state poisoned")?;
            if let Err(error) = outcome {
                state.issues.push(error);
            }
            let stem = test.name.trim_end_matches(".klfd");
            results.push(CaseResult {
                name: format!("{stem}::{name}"),
                mismatches: std::mem::take(&mut state.issues),
                skipped: false,
            });
        }
    }
    Ok(results)
}

fn test_sources(directory: &Path) -> Result<Vec<DefinitionSource>, String> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "klfd")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Ok(DefinitionSource::new(format!("tests/{name}"), text))
        })
        .collect()
}

fn register(
    sink: &mut ServiceSink,
    state: &Arc<Mutex<TestState>>,
    language: &Arc<LanguageDefinition>,
    target: &Target,
    package: &Path,
) {
    let root = package.to_path_buf();
    sink.register_state_with_memory(
        "klfsvc_test_source_path",
        state,
        move |state, words, memory| {
            let [path, length] = words else { return 0 };
            let path = read(memory, *path, *length);
            match std::fs::read_to_string(root.join(&path)) {
                Ok(text) => {
                    state.sources.push((path, text));
                    1
                }
                Err(error) => {
                    state.issues.push(format!("cannot read {path}: {error}"));
                    0
                }
            }
        },
    );
    sink.register_state_with_memory("klfsvc_test_expect", state, |state, words, memory| {
        let [passed, text, text_length, location, location_length] = words else {
            return 0;
        };
        if *passed == 0 {
            let text = read(memory, *text, *text_length);
            let text = match text.split_once('\n') {
                Some((first, _)) => format!("{first} ..."),
                None => text,
            };
            let location = read(memory, *location, *location_length);
            state
                .issues
                .push(format!("{location}: expectation failed: {text}"));
        }
        0
    });
    sink.register_state_with_memory("klfsvc_test_source", state, |state, words, memory| {
        let [name, name_length, text, text_length] = words else {
            return 0;
        };
        let name = read(memory, *name, *name_length);
        let text = read(memory, *text, *text_length);
        state.sources.push((name, text));
        0
    });
    register_checks(sink, state, language, target);
}

/// Registers the services a test compiles, runs and formats programs and
/// checks their diagnostics through.
fn register_checks(
    sink: &mut ServiceSink,
    state: &Arc<Mutex<TestState>>,
    language: &Arc<LanguageDefinition>,
    target: &Target,
) {
    let compiler = Arc::clone(language);
    let compile_target = target.clone();
    sink.register_state("klfsvc_test_compile", state, move |state, _| {
        u64::from(compile(state, &compiler, &compile_target).is_some())
    });
    let runner = Arc::clone(language);
    let run_target = target.clone();
    sink.register_state("klfsvc_test_run", state, move |state, _| {
        let Some(module) = compile(state, &runner, &run_target) else {
            let reported = state.diagnostics.join("; ");
            state
                .issues
                .push(format!("run: the program was rejected: {reported}"));
            return 0;
        };
        match crate::run(&module, "main", &[], Backend::Vm) {
            Ok(Outcome::Return(Some(value))) => value,
            Ok(other) => {
                state.issues.push(format!("run: main ended with {other:?}"));
                0
            }
            Err(error) => {
                state.issues.push(format!("run: {error}"));
                0
            }
        }
    });
    let formatter = Arc::clone(language);
    let format_target = target.clone();
    sink.register_state_with_memory("klfsvc_test_formats", state, move |state, words, memory| {
        let [source, source_length, expected, expected_length] = words else {
            return 0;
        };
        let source = read(memory, *source, *source_length);
        let expected = read(memory, *expected, *expected_length);
        u64::from(formats(
            state,
            &formatter,
            &format_target,
            &source,
            &expected,
        ))
    });
    sink.register_state("klfsvc_test_diagnosed", state, |state, words| {
        let [code, line, column] = words else {
            return 0;
        };
        let (code, line, column) = (code.cast_signed(), line.cast_signed(), column.cast_signed());
        u64::from(
            state.places.iter().any(|place| {
                place.0 == code && place.1 == line && (column == 0 || place.2 == column)
            }),
        )
    });
    sink.register_state_with_memory("klfsvc_test_reported", state, |state, words, memory| {
        let [text, length] = words else { return 0 };
        let text = read(memory, *text, *length);
        u64::from(
            state
                .diagnostics
                .iter()
                .any(|reported| reported.contains(&text)),
        )
    });
}

/// Whether the language formats `source` to `expected`, and leaves
/// `expected` as it is. A difference is an issue that shows the output.
fn formats(
    state: &mut TestState,
    language: &LanguageDefinition,
    target: &Target,
    source: &str,
    expected: &str,
) -> bool {
    let extension = language
        .descriptor()
        .extensions
        .first()
        .cloned()
        .unwrap_or_default();
    let name = format!("input.{extension}");
    let options = klf_format::Options::default();
    let format = |text: &str| {
        language
            .format_source(&mut Session::new(target.clone()), &name, text, &options)
            .map_err(|error| format!("formats: {error}"))
    };
    let checked = format(source).and_then(|formatted| {
        if formatted != expected {
            return Err(format!("formats: the output was\n{formatted}"));
        }
        let again = format(&formatted)?;
        if again == formatted {
            Ok(())
        } else {
            Err(format!("formats: formatting again gave\n{again}"))
        }
    });
    match checked {
        Ok(()) => true,
        Err(issue) => {
            state.issues.push(issue);
            false
        }
    }
}

fn read(memory: &dyn ServiceMemory, address: u64, length: u64) -> String {
    let mut bytes = vec![0; usize::try_from(length).unwrap_or(0)];
    if !bytes.is_empty() && !memory.read_buffer(address, &mut bytes) {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Compiles the files added since the last compilation, keeping what it
/// reported.
fn compile(
    state: &mut TestState,
    language: &LanguageDefinition,
    target: &Target,
) -> Option<klf_kir::Module> {
    let extension = language
        .descriptor()
        .extensions
        .first()
        .cloned()
        .unwrap_or_default();
    let sources = std::mem::take(&mut state.sources);
    let inputs: Vec<DefinitionSource> = sources
        .into_iter()
        .enumerate()
        .map(|(index, (name, text))| {
            let name = match (name.is_empty(), index) {
                (false, _) => name,
                (true, 0) => format!("input.{extension}"),
                (true, _) => format!("input{index}.{extension}"),
            };
            DefinitionSource::new(name, text)
        })
        .collect();
    let mut session = Session::new(target.clone());
    // The language's own driver flags read their defaults, as a build
    // with none given on the command line does.
    if let Err(error) = session.initialize_flags(&language.descriptor().flags) {
        state.diagnostics.clear();
        state.diagnostics.push(error);
        return None;
    }
    let module = language.compile_sources(&mut session, &inputs);
    state.diagnostics.clear();
    state.places.clear();
    for diagnostic in session.diagnostics() {
        let message = format!("{}: {}", diagnostic.code, diagnostic.message);
        state.places.extend(rendered_places(&message));
        if let (Ok(code), Some(span)) = (diagnostic.code.parse::<i64>(), diagnostic.primary_span) {
            let place = session.file(span.file).line_column(span.start);
            state
                .places
                .push((code, i64::from(place.line), i64::from(place.column)));
        }
        state.diagnostics.push(message);
    }
    module
}

/// Every `file:line:column: error[code]` a message renders, as
/// `(code, line, column)`: a phase that fails reports its diagnostics inside
/// one kernel diagnostic.
fn rendered_places(message: &str) -> Vec<(i64, i64, i64)> {
    message
        .match_indices(": error[")
        .filter_map(|(index, marker)| {
            let rest = &message[index + marker.len()..];
            let code = rest[..rest.find(']')?].parse().ok()?;
            let mut place = message[..index].rsplitn(3, ':');
            let column = place.next()?.parse().ok()?;
            let line = place.next()?.parse().ok()?;
            Some((code, line, column))
        })
        .collect()
}
