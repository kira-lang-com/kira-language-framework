//! Corpus manifests and the `klf test` runner (section 31.1).
//!
//! A manifest is `corpus/<name>.klfd` in the definition package, holding one
//! `construct <Name>() extends Corpus`:
//!
//! ```text
//! construct Smoke() extends Corpus {
//!     let oracle: [String] = []
//!     let compare: [String] = ["accept", "stdout", "exit"]
//!     let cases: [CorpusCase] = [
//!         CorpusCase { name = "square", sources = ["square/app.sq"], stdout = "RET 49\n" }
//!     ]
//! }
//! ```
//!
//! Each case builds its `sources` (relative to the manifest's directory) with
//! the generated driver as `<driver> <sources> <arguments> -o <program>`, run
//! in the manifest's directory. A
//! non-empty `oracle` is a command prefix given the same arguments; its
//! acceptance, stdout and exit code are the expectation. Without an oracle the
//! case's recorded `accept`, `diagnostics`, `stdout` and `exit` are. `compare`
//! picks what must agree: `accept`, `diagnostics` (`"<code>:<line>"` for each
//! `file:line:col: error[code]` or `warning[code]` the driver reports, `"<code>:0"` for one
//! without a place, recorded expectations only), `stdout` and `exit`. The program runs only when both sides accept.
//! A case's `cArchives` are C sources its package links as native libraries
//! (section 26.1): each is compiled by the bundled clang into `lib<stem>.a`
//! beside it before the case builds. Its `kiraArchives` are Kira library
//! packages, each built by the case's driver with `--emit lib` into
//! `lib<directory>.a` beside the package, for a case that imports them over
//! RIDL; its `kiraImages` are built the same way with `--emit klbc` into the
//! VM image `<directory>.klbc`, for a case whose program loads them as
//! plugins. A case whose `engine` is `"vm"` builds its program with `--emit
//! klbc` and runs the image with the language's runner. A case whose `hosts`
//! does not name this operating system (`std::env::consts::OS`) is skipped.
//!
//! An editor corpus, `corpus/editor/<name>.klfd` (`KLF-EDITOR.md`
//! Appendix A), runs against the same driver's `<driver> lsp`; see
//! `editor.rs`.
//!
//! A shader language's driver builds a case as `<driver> build <sources>
//! <arguments> --out <directory>`; its programs do not run on the host, so
//! only `accept` and `diagnostics` are compared.

mod editor;
mod editor_manifest;
mod editor_markers;
mod lsp_client;
mod manifest;

pub use editor::run_editor_corpus;
pub use editor_manifest::{
    EditorCase, EditorCheck, EditorFile, EditorManifest, read_editor_corpora,
};
use klf_services::Session;
pub use manifest::{Compared, CorpusCase, CorpusManifest, Engine, read_corpora};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What one build-and-run produced.
#[derive(Debug, Default, Eq, PartialEq)]
struct Observed {
    accept: bool,
    diagnostics: Vec<String>,
    rendered: String,
    stdout: String,
    exit: i64,
}

/// One case's verdict: empty `mismatches` is a pass.
#[derive(Debug)]
pub struct CaseResult {
    pub name: String,
    pub mismatches: Vec<String>,
    /// The case names hosts other than this one and did not run.
    pub skipped: bool,
}

/// `klf test`: builds the package's toolchain into a scratch sysroot, then
/// runs its corpus manifests (all of them, or only `only`).
///
/// # Errors
/// Returns a message when the manifests are malformed or the build fails.
pub fn test_package(
    session: &mut Session,
    package: &Path,
    only: Option<&str>,
    target: &klf_target::Target,
    languages: &[&std::path::Path],
) -> Result<Vec<CaseResult>, String> {
    let editor = read_editor_corpora(package, only)?;
    let manifests = if editor.is_empty() || has_program_corpus(package) {
        read_corpora(package, only)?
    } else {
        Vec::new()
    };
    let scratch = std::env::temp_dir().join(format!("klf-test-{}", std::process::id()));
    let linked = linked_languages(package, &manifests, languages);
    let linked: Vec<&Path> = linked.iter().map(PathBuf::as_path).collect();
    let built = crate::build_sysroot(
        session,
        package,
        &scratch.join("sysroot"),
        std::slice::from_ref(target),
        &linked,
        crate::Profile::Debug,
    )?;
    let shader = built.runner.as_os_str().is_empty();
    let mut results: Vec<CaseResult> = manifests
        .iter()
        .flat_map(|manifest| {
            if shader {
                run_shader_corpus(&built.driver, manifest, &scratch.join("cases"))
            } else {
                run_corpus(
                    &built.driver,
                    &built.runner,
                    manifest,
                    &scratch.join("cases"),
                )
            }
        })
        .collect();
    for manifest in &editor {
        results.extend(run_editor_corpus(
            &built.driver,
            manifest,
            &scratch.join("cases"),
        ));
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(results)
}

/// Whether `package` holds a corpus `klf test` runs: program manifests in
/// `corpus/` or editor manifests in `corpus/editor/`.
#[must_use]
pub fn has_corpus(package: &Path) -> bool {
    has_program_corpus(package) || package.join("corpus").join("editor").is_dir()
}

fn has_program_corpus(package: &Path) -> bool {
    std::fs::read_dir(package.join("corpus")).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "klfd")
        })
    })
}

/// Runs every case of `manifest` with `driver`, building into `scratch`;
/// `runner` runs the images of cases on the VM.
#[must_use]
/// Every shader language the driver these cases run against must hold: the
/// ones the caller named, then the ones each corpus names.
///
/// A case asking `--target wgsl` is answered by an out, and the out lives in
/// whichever language writes it. A corpus that does not say so reports the case
/// as one the toolchain cannot express, which is a statement about the driver it
/// was built rather than about the shader.
fn linked_languages(package: &Path, manifests: &[CorpusManifest], named: &[&Path]) -> Vec<PathBuf> {
    let mut linked: Vec<PathBuf> = named.iter().map(|path| (*path).to_path_buf()).collect();
    for manifest in manifests {
        for relative in &manifest.shader_languages {
            let path = package.join(relative);
            if !linked.contains(&path) {
                linked.push(path);
            }
        }
    }
    linked
}

#[must_use]
pub fn run_corpus(
    driver: &Path,
    runner: &Path,
    manifest: &CorpusManifest,
    scratch: &Path,
) -> Vec<CaseResult> {
    let runs = |case: &CorpusCase| {
        case.hosts.is_empty() || case.hosts.iter().any(|host| host == std::env::consts::OS)
    };
    // A Kira library is built beside its package, where every case naming it
    // reads it, so each is built once before the cases run side by side.
    let mut libraries = Vec::new();
    for case in manifest.cases.iter().filter(|case| runs(case)) {
        let packages = case.kira_archives.iter().map(|package| (package, Engine::Native));
        let images = case.kira_images.iter().map(|package| (package, Engine::Vm));
        for (package, engine) in packages.chain(images) {
            if !libraries.contains(&(package, engine)) {
                libraries.push((package, engine));
            }
        }
    }
    let mut failed = Vec::new();
    // A C archive is written beside its source too, so it is built once here
    // as well: cases building it side by side removed and rewrote one file
    // another case was linking.
    let mut archives: Vec<&String> = Vec::new();
    for case in manifest.cases.iter().filter(|case| runs(case)) {
        for source in &case.c_archives {
            if !archives.contains(&source) {
                archives.push(source);
            }
        }
    }
    let archive_scratch = scratch.join(&manifest.name).join("c-archives");
    for &source in &archives {
        let built = std::fs::create_dir_all(&archive_scratch)
            .map_err(|error| format!("cannot create {}: {error}", archive_scratch.display()))
            .and_then(|()| build_c_archive(&manifest.directory.join(source), &archive_scratch));
        if let Err(error) = built {
            failed.push((source, error));
        }
    }
    for &(package, engine) in &libraries {
        let built = build_kira_library(
            driver,
            &manifest.directory.join(package),
            &manifest.directory,
            engine,
        );
        if let Err(error) = built {
            failed.push((package, error));
        }
    }
    let one = |case: &CorpusCase| {
        let name = format!("{}::{}", manifest.name, case.name);
        if !runs(case) {
            return CaseResult { name, mismatches: Vec::new(), skipped: true };
        }
        let broken = case
            .kira_archives
            .iter()
            .chain(&case.kira_images)
            .chain(&case.c_archives)
            .find_map(|package| failed.iter().find(|(built, _)| *built == package));
        if let Some((_, error)) = broken {
            return CaseResult { name, mismatches: vec![error.clone()], skipped: false };
        }
        let directory = scratch.join(&manifest.name).join(&case.name);
        let mismatches = match run_case(driver, runner, manifest, case, &directory) {
            Ok(mismatches) => mismatches,
            Err(error) => vec![error],
        };
        CaseResult { name, mismatches, skipped: false }
    };
    // Each case is its own compiler and program processes writing under its
    // own directory, so the cases run on every core, reported in order.
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(manifest.cases.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<Option<CaseResult>> = Vec::new();
    results.resize_with(manifest.cases.len(), || None);
    let results = std::sync::Mutex::new(results);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(case) = manifest.cases.get(index) else { break };
                    let result = one(case);
                    if let Ok(mut results) = results.lock() {
                        results[index] = Some(result);
                    }
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .into_iter()
        .flatten()
        .collect()
}

fn run_case(
    driver: &Path,
    runner: &Path,
    manifest: &CorpusManifest,
    case: &CorpusCase,
    directory: &Path,
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let sources: Vec<PathBuf> = case
        .sources
        .iter()
        .map(|source| manifest.directory.join(source))
        .collect();
    let command = [driver.to_string_lossy().into_owned()];
    let observed = match case.engine {
        Engine::Native => observe(
            &command,
            &sources,
            &case.arguments,
            &directory.join("program"),
            &manifest.directory,
        )?,
        Engine::Vm => observe_on_vm(
            driver,
            runner,
            &sources,
            &case.arguments,
            &directory.join("program.klbc"),
            &manifest.directory,
        )?,
    };
    let expected = if manifest.oracle.is_empty() {
        Observed {
            accept: case.accept,
            diagnostics: case.diagnostics.clone(),
            rendered: case.rendered.clone(),
            stdout: case.stdout.clone(),
            exit: case.exit,
        }
    } else {
        observe(
            &manifest.oracle,
            &sources,
            &case.arguments,
            &directory.join("oracle"),
            &manifest.directory,
        )?
    };
    let mut mismatches = Vec::new();
    let both_ran = observed.accept && expected.accept;
    for compared in &manifest.compare {
        let (what, expected, actual) = match compared {
            Compared::Accept => (
                "accept",
                expected.accept.to_string(),
                observed.accept.to_string(),
            ),
            Compared::Diagnostics => (
                "diagnostics",
                expected.diagnostics.join(", "),
                observed.diagnostics.join(", "),
            ),
            Compared::Rendered => (
                "rendered",
                expected.rendered.clone(),
                observed.rendered.clone(),
            ),
            Compared::Stdout if both_ran => {
                ("stdout", expected.stdout.clone(), observed.stdout.clone())
            }
            Compared::Exit if both_ran => {
                ("exit", expected.exit.to_string(), observed.exit.to_string())
            }
            Compared::Stdout | Compared::Exit => continue,
        };
        if expected != actual {
            mismatches.push(format!("{what}: expected {expected:?}, got {actual:?}"));
            if what == "diagnostics" {
                // The compiler's own words, so a diagnostic the expectation did
                // not anticipate can be read where it came from.
                let head: Vec<&str> = observed.rendered.lines().take(6).collect();
                mismatches.push(format!("compiler said: {}", head.join(" | ")));
            }
        }
    }
    Ok(mismatches)
}

/// Runs every case of `manifest` with the shader driver `driver`, writing
/// each case's outputs under `scratch`.
#[must_use]
pub fn run_shader_corpus(
    driver: &Path,
    manifest: &CorpusManifest,
    scratch: &Path,
) -> Vec<CaseResult> {
    manifest
        .cases
        .iter()
        .map(|case| {
            let directory = scratch.join(&manifest.name).join(&case.name);
            let mismatches = match shader_case(driver, manifest, case, &directory) {
                Ok(mismatches) => mismatches,
                Err(error) => vec![error],
            };
            CaseResult {
                name: format!("{}::{}", manifest.name, case.name),
                mismatches,
                skipped: false,
            }
        })
        .collect()
}

fn shader_case(
    driver: &Path,
    manifest: &CorpusManifest,
    case: &CorpusCase,
    directory: &Path,
) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let build = Command::new(driver)
        .arg("build")
        .args(
            case.sources
                .iter()
                .map(|source| manifest.directory.join(source)),
        )
        .args(&case.arguments)
        .arg("--out")
        .arg(directory)
        .current_dir(&manifest.directory)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", driver.display()))?;
    let accept = build.status.success();
    let stderr = String::from_utf8_lossy(&build.stderr);
    let found = diagnostics(&stderr);
    let mut mismatches = Vec::new();
    for compared in &manifest.compare {
        let (what, expected, actual) = match compared {
            Compared::Accept => ("accept", case.accept.to_string(), accept.to_string()),
            Compared::Diagnostics => ("diagnostics", case.diagnostics.join(", "), found.join(", ")),
            Compared::Stdout | Compared::Exit | Compared::Rendered => continue,
        };
        if expected != actual {
            mismatches.push(format!("{what}: expected {expected:?}, got {actual:?}"));
        }
    }
    if !mismatches.is_empty() && found.is_empty() {
        let stdout = String::from_utf8_lossy(&build.stdout);
        let first = stderr
            .lines()
            .chain(stdout.lines())
            .next()
            .unwrap_or_default();
        mismatches.push(format!("driver: {} {first}", build.status));
    }
    Ok(mismatches)
}

/// Compiles the C source `source` into `lib<stem>.a` beside it.
fn build_c_archive(source: &Path, scratch: &Path) -> Result<(), String> {
    let clang = crate::kernel::find_clang().ok_or_else(|| "no bundled clang found".to_owned())?;
    let archiver = crate::kernel::find_llvm_tool("llvm-ar")
        .ok_or_else(|| "no bundled llvm-ar found".to_owned())?;
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("{} names no C source", source.display()))?;
    let object = scratch.join(format!("{stem}.o"));
    let archive = source.with_file_name(format!("lib{stem}.a"));
    let _ = std::fs::remove_file(&archive);
    let mut compile = vec!["-c".to_owned(), "-O2".to_owned()];
    compile.extend(klf_target::pic_flags(crate::host_triple()).iter().map(|flag| (*flag).to_owned()));
    // The bundled clang carries no C library headers; on macOS they are the
    // SDK's, as the native link takes them (`kernel::link`).
    if let Some(sysroot) = crate::native::macos_sysroot() {
        compile.extend(["-isysroot".to_owned(), sysroot]);
    }
    compile.extend([path_text(source), "-o".to_owned(), path_text(&object)]);
    for (tool, arguments) in [
        (&clang, compile),
        (
            &archiver,
            vec!["rcs".to_owned(), path_text(&archive), path_text(&object)],
        ),
    ] {
        let run = Command::new(tool)
            .args(&arguments)
            .output()
            .map_err(|error| format!("cannot run {}: {error}", tool.display()))?;
        if !run.status.success() {
            return Err(format!(
                "{} failed: {}",
                tool.display(),
                String::from_utf8_lossy(&run.stderr)
            ));
        }
    }
    Ok(())
}

/// Builds the Kira library package `package` with `driver` beside it: into
/// `lib<directory>.a` for native programs, or into the image
/// `<directory>.klbc` for the VM.
fn build_kira_library(
    driver: &Path,
    package: &Path,
    working_directory: &Path,
    engine: Engine,
) -> Result<(), String> {
    let stem = package
        .file_name()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("{} names no package", package.display()))?;
    let (emit, file_name) = match engine {
        Engine::Native => ("lib", format!("lib{stem}.a")),
        Engine::Vm => ("klbc", format!("{stem}.klbc")),
    };
    let library = package.with_file_name(file_name);
    let _ = std::fs::remove_file(&library);
    let run = Command::new(driver)
        .arg(package)
        .args(["--emit", emit, "-o"])
        .arg(&library)
        .current_dir(working_directory)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", driver.display()))?;
    if !run.status.success() {
        return Err(format!(
            "building the Kira library {} failed: {}{}",
            package.display(),
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ));
    }
    Ok(())
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Builds `sources` into the image `image` and runs it with `runner`.
fn observe_on_vm(
    driver: &Path,
    runner: &Path,
    sources: &[PathBuf],
    arguments: &[String],
    image: &Path,
    working_directory: &Path,
) -> Result<Observed, String> {
    let build = Command::new(driver)
        .args(sources)
        .args(arguments)
        .args(["--emit", "klbc", "-o"])
        .arg(image)
        .current_dir(working_directory)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", driver.display()))?;
    ran(&build, Command::new(runner).arg(image), working_directory)
}

fn observe(
    command: &[String],
    sources: &[PathBuf],
    arguments: &[String],
    program: &Path,
    working_directory: &Path,
) -> Result<Observed, String> {
    let (tool, prefix) = command
        .split_first()
        .ok_or_else(|| "empty build command".to_owned())?;
    let build = Command::new(tool)
        .args(prefix)
        .args(sources)
        .args(arguments)
        .arg("-o")
        .arg(program)
        .current_dir(working_directory)
        .output()
        .map_err(|error| format!("cannot run {tool}: {error}"))?;
    ran(&build, &mut Command::new(program), working_directory)
}

/// What a build reported and, when it succeeded, what `program` printed and
/// the code it exited with.
fn ran(build: &Output, program: &mut Command, root: &Path) -> Result<Observed, String> {
    let stderr = String::from_utf8_lossy(&build.stderr);
    let mut observed = Observed {
        accept: build.status.success(),
        diagnostics: diagnostics(&stderr),
        rendered: normalized(&stderr, root),
        ..Observed::default()
    };
    if observed.accept {
        let run = program.output().map_err(|error| {
            format!(
                "cannot run {}: {error}",
                program.get_program().to_string_lossy()
            )
        })?;
        observed.stdout = String::from_utf8_lossy(&run.stdout).into_owned();
        observed.exit = i64::from(run.status.code().unwrap_or(-1));
        // A Unix status keeps a program's result to its low eight bits, and
        // the expectations are written so; Windows keeps all thirty-two. A
        // negative one there is the system ending the program, kept whole.
        if cfg!(windows) && observed.exit >= 0 {
            observed.exit &= 0xff;
        }
    }
    Ok(observed)
}

/// The text a build printed, with trailing blanks dropped from each line, the
/// working directory's prefix removed from paths, and the whole trimmed, so a
/// recorded expectation holds wherever the package is checked out.
fn normalized(text: &str, root: &Path) -> String {
    // Windows writes a path with `\`; the expectations are written with `/`.
    let slashed = |text: String| if cfg!(windows) { text.replace('\\', "/") } else { text };
    let prefix = slashed(format!("{}/", root.display()));
    slashed(text.to_owned())
        .replace(&prefix, "")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// `"<code>:<line>"` for every `file:line:col: error[code]` or
/// `file:line:col: warning[code]` line, and `"<code>:0"` for one about no
/// place in a source, such as a target refusing a whole module.
fn diagnostics(stderr: &str) -> Vec<String> {
    stderr.lines().filter_map(diagnostic).collect()
}

fn diagnostic(line: &str) -> Option<String> {
    for severity in ["error[", "warning["] {
        if let Some(rest) = line.strip_prefix(severity) {
            let code = rest.split_once(']')?.0;
            return Some(format!("{code}:0"));
        }
        let Some((location, rest)) = line.split_once(&format!(": {severity}")) else {
            continue;
        };
        let code = rest.split_once(']')?.0;
        let mut parts = location.rsplitn(3, ':');
        let _column = parts.next()?;
        let line_number = parts.next()?;
        line_number.parse::<u64>().ok()?;
        return Some(format!("{code}:{line_number}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::normalized;
    use std::path::Path;

    #[test]
    fn rendered_text_drops_the_root_and_trailing_blanks() {
        let root = Path::new("/work/pkg");
        let text = "/work/pkg/app/main.kira:5:21: error[3061]: undefined  \n  |\n";
        assert_eq!(
            normalized(text, root),
            "app/main.kira:5:21: error[3061]: undefined\n  |"
        );
    }

    #[test]
    fn diagnostics_read_code_and_line() {
        assert_eq!(
            super::diagnostics(
                "in.c:3:9: error[310]: cast\nnote\nx.c:12:1: error[300]: y\nerror[237]: z"
            ),
            ["310:3", "300:12", "237:0"]
        );
    }

    #[test]
    fn diagnostics_read_warnings() {
        assert_eq!(
            super::diagnostics("a.kira:5:10: warning[6002]: prefixed\nin.c:1:1: warning: unused"),
            ["6002:5"]
        );
    }
}
