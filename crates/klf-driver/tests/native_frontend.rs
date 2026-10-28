//! `klf build --emit frontend` turns a definition package into a native
//! compiler driver, and what that driver emits matches the VM-hosted path.
//!
//! The VM is the oracle: section 7 fixes what a `Language` means, and the
//! VM-hosted driver is the reference reading of it. A native frontend runs the
//! same schedule over the same service bridge with the phase calls compiled, so
//! the KIR it writes has to be the KIR the VM produced, byte for byte.

use klf_driver::{Backend, LanguageDefinition, build_frontend, toolchain_available};
use klf_services::{DefinitionSource, Session};
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn session() -> Session {
    Session::new("aarch64-apple-darwin".parse().expect("valid target"))
}

/// The KIR the VM-hosted driver produces for `source`.
fn hosted(package: &Path, name: &str, source: &str) -> String {
    let mut session = session();
    let language = LanguageDefinition::load(&mut session, package, Backend::Vm)
        .unwrap_or_else(|| panic!("definition loads: {:?}", session.diagnostics()));
    let module = language
        .compile_sources(&mut session, &[DefinitionSource::new(name, source)])
        .unwrap_or_else(|| panic!("definition compiles: {:?}", session.diagnostics()));
    assert_eq!(session.diagnostics(), []);
    module.to_string()
}

/// The KIR a native frontend built from the same package writes for `source`.
fn native(package: &Path, name: &str, source: &str, tag: &str) -> String {
    let directory = std::env::temp_dir().join(format!("klf-frontend-test-{tag}"));
    std::fs::create_dir_all(&directory).expect("temp directory");
    let binary = directory.join(tag);

    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));

    let input = directory.join(name);
    std::fs::write(&input, source).expect("input written");
    let output = directory.join(format!("{tag}.kir"));
    let run = Command::new(&binary)
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("frontend runs");
    assert!(
        run.status.success(),
        "frontend failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&output).expect("emitted KIR")
}

#[test]
fn native_frontend_matches_the_vm_for_mini() {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let package = workspace().join("definitions/mini");
    let source = "answer 40";
    let emitted = native(&package, "input.mini", source, "mini");
    assert_eq!(emitted, hosted(&package, "input.mini", source));

    let module = klf_kir::parse_module(&emitted).expect("emitted KIR parses");
    assert!(
        klf_kir_verify::verify_module(&module).is_empty(),
        "emitted KIR fails the raw verifier"
    );
    assert_eq!(
        klf_driver::run(&module, "answer", &[], Backend::Vm).expect("runs"),
        klf_vm::Outcome::Return(Some(42))
    );
}

#[test]
fn native_frontend_emits_runnable_klbc() {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let package = workspace().join("definitions/mini");
    let directory = std::env::temp_dir().join("klf-frontend-test-klbc");
    std::fs::create_dir_all(&directory).expect("temp directory");
    let binary = directory.join("mini");
    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        &package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));

    let input = directory.join("input.mini");
    std::fs::write(&input, "answer 40").expect("input written");
    let run = Command::new(&binary)
        .arg(&input)
        .arg("--emit")
        .arg("klbc")
        .output()
        .expect("frontend runs");
    assert!(
        run.status.success(),
        "frontend failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let bytes = std::fs::read(directory.join("input.klbc")).expect("emitted bytecode");
    let program = klf_vm::decode(&bytes).expect("bytecode decodes");
    let entry = program.function_index("answer").expect("answer entry");
    assert_eq!(
        klf_vm::run(&program, entry, &[]),
        klf_vm::Outcome::Return(Some(42))
    );
}

/// `--ridl-source` marks each function of an emitted `.ridl` with where it
/// was named, relative to the `.ridl` (`KLF-EDITOR.md` section 15.3), and
/// leaves the surface without it unmarked.
#[test]
fn native_frontend_marks_ridl_with_sources() {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let package = workspace().join("definitions/mini");
    let directory = std::env::temp_dir().join("klf-frontend-test-ridl-source");
    std::fs::create_dir_all(directory.join("out")).expect("temp directory");
    let binary = directory.join("mini");
    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        &package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));
    let input = directory.join("input.mini");
    std::fs::write(&input, "answer 40").expect("input written");
    let emit = |output: &str, marked: bool| {
        let mut command = Command::new(&binary);
        command
            .args(["build"])
            .arg(&input)
            .args(["--emit", "ridl", "-o", output]);
        if marked {
            command.arg("--ridl-source");
        }
        let run = command
            .current_dir(&directory)
            .output()
            .expect("frontend runs");
        assert!(
            run.status.success(),
            "frontend failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::read_to_string(directory.join(output)).expect("emitted RIDL")
    };
    let plain = emit("out/plain.ridl", false);
    let marked = emit("out/marked.ridl", true);
    assert!(!plain.contains("@Source"), "{plain}");
    assert!(
        marked.contains("@Source(\"../input.mini:1:1\")\nfunction answer"),
        "{marked}"
    );
    let canonical = |text: &str| {
        let parsed = klf_ridl::parse(klf_core::FileId::from_u32(0), "input", text);
        klf_ridl::canonical_text(&parsed.expect("emitted RIDL parses"))
    };
    assert_eq!(canonical(&plain), canonical(&marked));
}

/// `<driver> lsp-worker` analyzes the open document, not the file on disk,
/// and returns what the phases recorded.
#[test]
#[allow(clippy::too_many_lines)]
fn native_frontend_analyzes_for_the_language_server() {
    use klf_facts::{Entity, TokenClass};
    use klf_ide::{AnalysisRequest, Analyzer, Cancellation, ProcessAnalyzer};

    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let package = workspace().join("definitions/mini");
    let directory = std::env::temp_dir().join("klf-frontend-test-lsp");
    std::fs::create_dir_all(&directory).expect("temp directory");
    let binary = directory.join("mini");
    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        &package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));

    let input = directory.join("input.mini");
    std::fs::write(&input, "answer 40").expect("input written");
    let analyzer = ProcessAnalyzer {
        program: binary,
        arguments: vec!["lsp-worker".to_owned()],
    };
    let analysis = analyzer.analyze(
        &AnalysisRequest {
            directory: directory.clone(),
            arguments: vec![input.display().to_string()],
            documents: vec![(input.clone(), "total 7".to_owned())],
            kir: false,
        },
        &Cancellation::new(),
    );
    assert_eq!(analysis.failure, None, "{:?}", analysis.diagnostics);
    let file = analysis
        .file(&input)
        .expect("the input is a file of the analysis");
    let symbol = analysis
        .facts
        .symbol(Entity(1))
        .expect("mini declares its entry");
    assert_eq!(
        symbol.name, "total",
        "the open document's text, not the disk's"
    );
    let tokens = analysis.facts.tokens(file);
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        (tokens[0].start, tokens[0].end, tokens[0].class),
        (0, 5, TokenClass::Function)
    );
    let info = |analysis: &klf_ide::Analysis, path: &Path| {
        let file = analysis.file(path).expect("a file of the analysis");
        let info = analysis
            .file_info
            .iter()
            .find(|info| info.file == file)
            .expect("the file's mode is recorded");
        (
            info.mode.clone(),
            info.memory.clone(),
            info.convention.clone(),
            info.unit.is_some(),
        )
    };
    let plain = (
        String::new(),
        "Unmanaged".to_owned(),
        "Ridl".to_owned(),
        true,
    );
    assert_eq!(info(&analysis, &input), plain, "the language's defaults");
    let counted = directory.join("input.minic");
    std::fs::write(&counted, "answer 1").expect("input written");
    let analysis = analyzer.analyze(
        &AnalysisRequest {
            directory: directory.clone(),
            arguments: vec![counted.display().to_string()],
            documents: Vec::new(),
            kir: true,
        },
        &Cancellation::new(),
    );
    let mode = (
        "counted".to_owned(),
        "Counted".to_owned(),
        "Platform".to_owned(),
        true,
    );
    assert_eq!(info(&analysis, &counted), mode, "the `.minic` file mode");
    let file = analysis.file(&counted).expect("a file of the analysis");
    let function = analysis
        .kir
        .iter()
        .find(|function| function.file == file)
        .expect("asked for, the KIR of the file's functions is recorded");
    assert!(function.raw.starts_with("func "), "{}", function.raw);
    assert!(
        function.canonical.starts_with("func "),
        "{}",
        function.canonical
    );
}

/// `<driver> format` in a native frontend runs the definition's compiled
/// `format` hook and answers what the VM-hosted formatter answers: on
/// standard input, in place, and under `--check`.
#[test]
fn native_frontend_formats_as_the_vm_does() {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let directory = std::env::temp_dir().join("klf-frontend-test-format");
    let package = directory.join("lists");
    std::fs::create_dir_all(&package).expect("temp directory");
    std::fs::write(package.join("language.klfd"), include_str!("lists.klfd"))
        .expect("definition written");
    let binary = directory.join("lists-driver");
    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        &package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));

    let format_stdin = |source: &str, width: &str| {
        let mut child = Command::new(&binary)
            .args(["format", "--stdin", "input.lists", "--width", width])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("frontend runs");
        std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), source.as_bytes())
            .expect("source written");
        let output = child.wait_with_output().expect("frontend finishes");
        assert!(
            output.status.success(),
            "format failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 output")
    };
    assert_eq!(
        format_stdin("[1,2,  [3 ,4]]\n\n\n[5]", "80"),
        "[1, 2, [3, 4]]\n\n[5]\n"
    );
    assert_eq!(
        format_stdin("[10, 20, [30, 40]]", "12"),
        "[\n    10,\n    20,\n    [30, 40]\n]\n"
    );
    assert_eq!(
        format_stdin("// lists\n[1, // one\n2]", "80"),
        "// lists\n[\n    1, // one\n    2\n]\n"
    );

    let file = directory.join("input.lists");
    std::fs::write(&file, "[1,2,  [3 ,4]]").expect("input written");
    let check = Command::new(&binary)
        .arg("format")
        .arg("--check")
        .arg(&file)
        .output()
        .expect("frontend runs");
    assert_eq!(
        check.status.code(),
        Some(1),
        "an unformatted file fails --check"
    );
    assert_eq!(
        String::from_utf8_lossy(&check.stdout).trim(),
        file.display().to_string()
    );
    let in_place = Command::new(&binary)
        .arg("format")
        .arg(&file)
        .status()
        .expect("frontend runs");
    assert!(in_place.success());
    assert_eq!(
        std::fs::read_to_string(&file).expect("formatted file"),
        "[1, 2, [3, 4]]\n"
    );
    let again = Command::new(&binary)
        .arg("format")
        .arg("--check")
        .arg(&file)
        .status()
        .expect("frontend runs");
    assert!(again.success(), "a formatted file passes --check");
}

const NAMES_CORPUS: &str = r#"construct NamesEditor() extends EditorCorpus {
    let cases: [EditorCase] = [
        EditorCase {
            name = "navigation",
            files = [EditorFile { path = "main.names", text = "let $<dx>x\nuse $[ux$0x$]\n$<l3>use y\nlet y\n$<end>" }],
            checks = [
                .Diagnostics(expected: ["3001:3"]),
                .Definition(at: "$0", target: "dx"),
                .References(at: "$0", targets: ["dx", "ux"], includeDeclaration: true),
                .References("$0", ["ux"], false),
                .Hover(at: "$0", contains: ["x", "a name"]),
                .Tokens(golden: "names.tokens"),
                .Folds(ranges: []),
                .Rename(at: "$0", newName: "z", expect: [EditorFile { path = "main.names", text = "let z\nuse z\nuse y\nlet y\n" }]),
                .Edit(at: "l3", insert: "let y\n", then: [.Diagnostics(expected: [])]),
                .Edit(at: "end", insert: "use $<c>", then: [.Completes(at: "c", includes: ["x", "y"], excludes: ["z"])]),
                .Diagnostics(expected: ["3001:3"])
            ]
        },
        EditorCase {
            name = "format",
            files = [EditorFile { path = "main.names", text = "let   x\n\n\nuse    x" }],
            checks = [.Format(expect: [EditorFile { path = "main.names", text = "let x\n\nuse x\n" }])]
        },
        EditorCase {
            name = "wrong",
            files = [EditorFile { path = "main.names", text = "let $<dx>x\nuse $[ux$0x$]\n" }],
            checks = [.Definition(at: "$0", target: "ux"), .Symbols(golden: "outline")]
        }
    ]
}
"#;

const NAMES_TOKENS: &str = "1:1 3 keyword
1:5 1 variable declaration
2:1 3 keyword
2:5 1 variable
3:1 3 keyword
3:5 1 variable
4:1 3 keyword
4:5 1 variable declaration
";

/// The editor corpus runner (`KLF-EDITOR.md` Appendix A) drives a native
/// frontend's `<driver> lsp` over the protocol: every check of a correct
/// case passes, and a wrong expectation and a check whose golden format
/// the appendix leaves open fail with their reasons.
#[test]
fn the_editor_corpus_runs_against_the_drivers_language_server() {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return;
    }
    let directory = std::env::temp_dir().join("klf-frontend-test-editor-corpus");
    let package = directory.join("names");
    let corpus = package.join("corpus/editor");
    std::fs::create_dir_all(&corpus).expect("temp directory");
    std::fs::write(package.join("language.klfd"), include_str!("names.klfd"))
        .expect("definition written");
    std::fs::write(corpus.join("names.klfd"), NAMES_CORPUS).expect("corpus written");
    std::fs::write(corpus.join("names.tokens"), NAMES_TOKENS).expect("golden written");
    assert!(klf_driver::has_corpus(&package));
    let binary = directory.join("names-driver");
    let mut session = session();
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    build_frontend(
        &mut session,
        &package,
        Some(&binary),
        &target,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("frontend builds: {error}"));

    let manifests = klf_driver::read_editor_corpora(&package, None).expect("the corpus reads");
    assert_eq!(manifests.len(), 1);
    let results = klf_driver::run_editor_corpus(&binary, &manifests[0], &directory.join("cases"));
    let outcome: Vec<(String, Vec<String>)> = results
        .into_iter()
        .map(|result| (result.name, result.mismatches))
        .collect();
    assert_eq!(
        outcome[0],
        ("NamesEditor::navigation".to_owned(), Vec::new())
    );
    assert_eq!(outcome[1], ("NamesEditor::format".to_owned(), Vec::new()));
    assert_eq!(outcome[2].0, "NamesEditor::wrong");
    assert_eq!(outcome[2].1.len(), 2, "{:?}", outcome[2].1);
    assert!(
        outcome[2].1[0].starts_with("definition at `$0`: expected `ux`, got [main.names:1:5]"),
        "{}",
        outcome[2].1[0]
    );
    assert!(outcome[2].1[1].contains("no format"), "{}", outcome[2].1[1]);
}
