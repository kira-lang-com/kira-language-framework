//! `klf test <defdir> [--unit] [--corpus <name>] [--language <pkg>]` (section
//! 5.2): run the package's `@Test` functions on the VM (`KLF-TESTING.md`),
//! then build its toolchain and run its corpus manifests (section 31.1).
//! `--unit` runs only the tests, `--corpus` only that manifest, and each
//! `--language` names another shader language for the built driver to reach an
//! out through.

use std::path::Path;
use std::process::ExitCode;

pub(crate) fn command(args: &[String]) -> ExitCode {
    let mut package = None;
    let mut corpus = None;
    let mut unit_only = false;
    let mut languages: Vec<String> = Vec::new();
    let mut timings = false;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--unit" => unit_only = true,
            "--timings" => timings = true,
            "--corpus" => match rest.next() {
                Some(name) => corpus = Some(name.clone()),
                None => return fail("`--corpus` needs a name"),
            },
            "--language" => match rest.next() {
                Some(path) => languages.push(path.clone()),
                None => return fail("`--language` needs a package path"),
            },
            other if package.is_none() && !other.starts_with('-') => {
                package = Some(other.to_owned());
            }
            other => return fail(&format!("unexpected argument `{other}`")),
        }
    }
    let Some(package) = package else {
        return fail(
            "usage: klf test <definition-package-dir> [--unit] [--corpus <name>] \
             [--language <package>] [--timings]",
        );
    };
    let mut finish = klf_diag::status::begin(&format!("Testing {package}"), timings);
    let target: klf_target::Target = match crate::default_target().parse() {
        Ok(target) => target,
        Err(_) => return fail("invalid default target"),
    };
    let mut session = klf_services::Session::new(target.clone());
    let package = Path::new(&package);
    let mut results = Vec::new();
    if corpus.is_none() {
        match klf_driver::run_unit_tests(&mut session, package) {
            Ok(tests) => results.extend(tests),
            Err(message) => {
                // Why the package did not compile with its tests.
                klf_diag::raw_err!("{}", crate::pipeline::render(&session));
                return fail(&message);
            }
        }
    }
    if !unit_only && klf_driver::has_corpus(package) {
        let others: Vec<&Path> = languages.iter().map(Path::new).collect();
        match klf_driver::test_package(
            &mut session,
            package,
            corpus.as_deref(),
            &target,
            &others,
        ) {
            Ok(cases) => results.extend(cases),
            Err(message) => return fail(&message),
        }
    }
    let failed = results
        .iter()
        .filter(|result| !result.mismatches.is_empty())
        .count();
    finish.end();
    let skipped = results.iter().filter(|result| result.skipped).count();
    for result in &results {
        if result.skipped {
            klf_diag::out!("SKIP {}", result.name);
        } else if result.mismatches.is_empty() {
            klf_diag::out!("PASS {}", result.name);
        } else {
            klf_diag::out!("FAIL {}", result.name);
            for mismatch in &result.mismatches {
                klf_diag::out!("  {mismatch}");
            }
        }
    }
    if skipped == 0 {
        klf_diag::out!("{} passed, {failed} failed", results.len() - failed);
    } else {
        klf_diag::out!("{} passed, {failed} failed, {skipped} skipped", results.len() - failed - skipped);
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("klf: {message}");
    ExitCode::FAILURE
}
