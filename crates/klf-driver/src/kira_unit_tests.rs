//! Running a Kira frontend package's own tests.

use crate::corpus::CaseResult;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch {
    directory: PathBuf,
    driver: PathBuf,
    native: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::Scratch;

    #[test]
    fn scratch_removes_bytecode_companions_and_nested_sources() {
        let directory =
            std::env::temp_dir().join(format!("klf-kira-cleanup-{}", std::process::id()));
        std::fs::create_dir(&directory).expect("create scratch directory");
        std::fs::write(directory.join("program.klbc.natives.dll"), b"library").unwrap();
        std::fs::create_dir(directory.join("nested")).unwrap();
        std::fs::write(directory.join("nested/input.cpp"), b"source").unwrap();
        let scratch = Scratch {
            driver: directory.join("frontend"),
            native: directory.join("program"),
            directory: directory.clone(),
        };
        drop(scratch);
        assert!(!directory.exists());
    }
}

pub(crate) fn run(
    session: &mut klf_services::Session,
    package: &Path,
) -> Result<Vec<CaseResult>, String> {
    let home = crate::kernel::archive_home().ok_or("no KLF distribution for the Kira frontend")?;
    let library = home.join("share/klf/KLF");
    if !library.join("package.kira").is_file() {
        return Err(format!(
            "{} does not contain the KLF Kira package",
            home.display()
        ));
    }
    let compiler = std::env::var_os("KLF_KIRA").unwrap_or_else(|| "kira".into());
    let dependency = format!("KLF={}", library.to_string_lossy().replace('\\', "/"));
    let (_, definition) = crate::kira_frontend::read_package(session, package)?;
    let extension = definition
        .descriptor
        .extensions
        .first()
        .ok_or("frontend has no source extension")?;
    if extension.is_empty()
        || extension.contains(['/', '\\'])
        || extension == "."
        || extension == ".."
    {
        return Err(format!("invalid frontend source extension `{extension}`"));
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "klf-kira-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create {}: {error}", scratch.display()))?;
    let scratch = Scratch {
        driver: scratch.join(klf_target::executable("frontend")),
        native: scratch.join(klf_target::executable("check-program")),
        directory: scratch,
    };
    let target = session.target().clone();
    crate::build_frontend(
        session,
        package,
        Some(&scratch.driver),
        &target,
        &[],
        crate::Profile::Debug,
    )?;
    let runner = crate::kernel::runner(crate::host_triple())?;
    let output = Command::new(compiler)
        .arg("test")
        .arg(package)
        .args(["--dependency", &dependency])
        .env("KLF_HOME", home)
        .env("KLF_TEST_DRIVER", &scratch.driver)
        .env("KLF_TEST_RUNNER", runner)
        .env("KLF_TEST_DIR", &scratch.directory)
        .env("KLF_TEST_EXTENSION", extension)
        .env("KLF_TEST_NATIVE", &scratch.native)
        .env(
            "KLF_TEST_PACKAGE",
            klf_target::plain_path(
                std::fs::canonicalize(package).map_err(|error| error.to_string())?,
            ),
        )
        .output()
        .map_err(|error| format!("cannot start Kira test runner: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut results: Vec<CaseResult> = Vec::new();
    let mut saw_summary = false;
    for line in stdout.lines() {
        if line.contains(" passed, ") && line.ends_with(" total") {
            saw_summary = true;
        }
        if let Some(name) = line.strip_prefix("ok   ") {
            results.push(CaseResult {
                name: name.to_owned(),
                mismatches: Vec::new(),
                skipped: false,
            });
        } else if let Some(name) = line.strip_prefix("FAIL ") {
            results.push(CaseResult {
                name: name.to_owned(),
                mismatches: vec!["Kira test failed".into()],
                skipped: false,
            });
        } else if let Some(name) = line.strip_prefix("skip ") {
            results.push(CaseResult {
                name: name.to_owned(),
                mismatches: Vec::new(),
                skipped: true,
            });
        } else if let Some(result) = results
            .last_mut()
            .filter(|result| !result.mismatches.is_empty())
        {
            if !line.trim().is_empty() && !line.contains(" passed, ") {
                result.mismatches.push(line.trim().to_owned());
            }
        }
    }
    if !saw_summary
        || (!output.status.success() && !results.iter().any(|result| !result.mismatches.is_empty()))
    {
        return Err(format!("Kira tests could not run\n{stdout}{stderr}"));
    }
    Ok(results)
}
