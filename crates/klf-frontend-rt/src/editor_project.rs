//! Isolates a frontend's project hook from the language server process.

use crate::{FrontendMetadata, NativePhases, klffe_phase, new_session};
use klf_defsvc::{ProjectCall, ServiceSink};
use klf_ide::{Project, ProjectDiagnostic, ProjectError, ProjectResolution};
use klf_native_rt::{interrupt, services};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(crate) fn resolve(program: &Path, path: &Path) -> Result<ProjectResolution, ProjectError> {
    let mut child = Command::new(program)
        .arg("lsp-project-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start project worker: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("project worker has no input")?
        .write_all(path.to_string_lossy().as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()
            .into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    let failed = value["failed"]
        .as_bool()
        .ok_or("project result has no failure status")?;
    let diagnostics = value["diagnostics"]
        .as_array()
        .ok_or("project result has no diagnostics")?
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic["severity"].as_str() {
                Some("error") => klf_diag::Severity::Error,
                Some("warning") => klf_diag::Severity::Warning,
                Some("note") => klf_diag::Severity::Note,
                Some("remark") => klf_diag::Severity::Remark,
                _ => return Err("project diagnostic has invalid severity"),
            };
            let span = diagnostic["start"]
                .as_u64()
                .and_then(|start| u32::try_from(start).ok())
                .zip(
                    diagnostic["end"]
                        .as_u64()
                        .and_then(|end| u32::try_from(end).ok()),
                );
            Ok(ProjectDiagnostic {
                message: diagnostic["message"]
                    .as_str()
                    .ok_or("project diagnostic has no message")?
                    .to_owned(),
                code: diagnostic["code"].as_i64(),
                span,
                path: diagnostic["path"].as_str().map(PathBuf::from),
                severity,
            })
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let value = &value["project"];
    if value.is_null() {
        return Ok(ProjectResolution {
            project: None,
            diagnostics,
            failed,
        });
    }
    let root = value["root"].as_str().ok_or("project has no root")?;
    let arguments = value["arguments"]
        .as_array()
        .ok_or("project has no arguments")?
        .iter()
        .map(|argument| {
            argument
                .as_str()
                .map(str::to_owned)
                .ok_or("project argument is not text")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let watch = value["watch"]
        .as_array()
        .ok_or("project has no watch paths")?
        .iter()
        .map(|path| {
            path.as_str()
                .map(str::to_owned)
                .ok_or("project watch path is not text")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let name = value["name"]
        .as_str()
        .ok_or("project has no name")?
        .to_owned();
    let output = match &value["output"] {
        serde_json::Value::Null => None,
        value => Some(PathBuf::from(
            value.as_str().ok_or("project output is not text")?,
        )),
    };
    Ok(ProjectResolution {
        project: Some(Project {
            name,
            root: PathBuf::from(root),
            arguments,
            loose: false,
            watch,
            output,
        }),
        diagnostics,
        failed,
    })
}

pub(crate) fn worker(metadata: &FrontendMetadata) -> i32 {
    let mut path = String::new();
    let result = std::io::stdin()
        .read_to_string(&mut path)
        .map_err(|error| error.to_string())
        .and_then(|_| call(metadata, &path));
    match result {
        Ok(project) => {
            println!("{}", project);
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn call(metadata: &FrontendMetadata, path: &str) -> Result<serde_json::Value, String> {
    let invocation = crate::arguments::parse_arguments(&[], metadata)?;
    let mut session = new_session(metadata, &invocation, &metadata.target)?;
    session.set_mode(klf_facts::AnalysisMode::Editor);
    let call = ProjectCall::open(&mut session, path)?;
    let mut sink = ServiceSink::new();
    call.register(&mut sink);
    services::install(sink.into_table(), call.source.clone());
    let frame = NativePhases::new();
    // SAFETY: selector 9 uses the two words prepared by ProjectCall.
    let result = interrupt::catching(|| unsafe {
        klffe_phase(9, call.words[0] as i64, call.words[1] as i64, 0)
    });
    drop(frame);
    let reported = services::uninstall();
    let word = result.map_err(|error| format!("project hook stopped: {error:?}"))?;
    let has_error = reported.iter().any(|record| {
        metadata
            .diagnostics
            .iter()
            .find(|spec| spec.code == record.code)
            .is_none_or(|spec| spec.severity == klf_diag::Severity::Error)
    });
    let (project, failed) = match call.finish(&mut session, &reported, word as u64) {
        Ok(project) => (project, false),
        Err(_) if has_error => (None, true),
        Err(message) => return Err(message),
    };
    let diagnostics: Vec<_> = reported
        .iter()
        .map(|record| {
            let spec = metadata
                .diagnostics
                .iter()
                .find(|spec| spec.code == record.code);
            let message = spec.map_or_else(
                || format!("unregistered diagnostic {}", record.code),
                |spec| format!("{}: {}", spec.title, spec.explanation),
            );
            let severity = match spec.map(|spec| spec.severity) {
                Some(klf_diag::Severity::Warning) => "warning",
                Some(klf_diag::Severity::Note) => "note",
                Some(klf_diag::Severity::Remark) => "remark",
                _ => "error",
            };
            let source = if record.origin > 0 {
                session
                    .files()
                    .iter()
                    .find(|source| i64::try_from(source.hash()) == Ok(record.origin))
            } else {
                usize::try_from(record.file)
                    .ok()
                    .and_then(|id| session.files().get(id))
            };
            let source_path = source
                .map(|source| PathBuf::from(source.name().as_str()))
                .map(|path| {
                    if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir().unwrap_or_default().join(path)
                    }
                });
            let span = ((source.is_some() || (record.file < 0 && record.origin == 0))
                && record.start >= 0
                && record.end >= record.start)
                .then_some((record.start, record.end));
            serde_json::json!({ "message": message, "code": record.code, "severity": severity,
                "path": source_path,
                "start": span.map(|span| span.0), "end": span.map(|span| span.1) })
        })
        .collect();
    let project = match project {
        None => serde_json::Value::Null,
        Some(project) => serde_json::json!({
            "name": project.name, "root": project.root, "arguments": project.arguments,
            "watch": project.watch, "output": project.output
        }),
    };
    Ok(serde_json::json!({ "project": project, "diagnostics": diagnostics, "failed": failed }))
}
