//! Running an analysis (`KLF-EDITOR.md` sections 8.5 and 8.6).
//!
//! A generated driver whose definition code can be unwound analyzes in its
//! own process. Where it cannot, it analyzes in a worker process: a trap in
//! definition code ends the worker, not the server, and cancelling an
//! analysis kills it.

use crate::analysis::{Analysis, AnalysisRequest, Interruption};
use crate::cancel::Cancellation;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Something that analyzes a project. `cancellation` turns when the result
/// is no longer wanted; an analyzer that notices returns early with
/// [`Interruption::Cancelled`].
pub trait Analyzer: Send + Sync {
    fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis;

    /// How many analyses may run at once. Definition code that shares
    /// process-wide state runs one at a time.
    fn lanes(&self) -> usize {
        1
    }
}

/// Analyzes by running `program arguments...` with the request on standard
/// input and reading the analysis from standard output.
pub struct ProcessAnalyzer {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}

const POLL: Duration = Duration::from_millis(2);

impl Analyzer for ProcessAnalyzer {
    fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis {
        let child = Command::new(&self.program)
            .args(&self.arguments)
            .current_dir(&request.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                return Analysis::failed(format!(
                    "cannot start {}: {error}",
                    self.program.display()
                ));
            }
        };
        let input = request.encode();
        let stdin = child.stdin.take();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(input.as_bytes());
            }
        });
        let reader = drain(child.stdout.take());
        let errors = drain(child.stderr.take());
        let status = loop {
            if cancellation.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                return Analysis::interrupted(Interruption::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(POLL),
                Err(error) => return Analysis::failed(format!("analysis worker: {error}")),
            }
        };
        let _ = writer.join();
        let output = reader.join().unwrap_or_default();
        let errors = errors.join().unwrap_or_default();
        match Analysis::decode(&output) {
            Ok(analysis) if !output.is_empty() => analysis,
            _ => Analysis::interrupted(Interruption::Trapped {
                code: format!("worker {status}"),
                query: errors.lines().last().unwrap_or("no output").to_owned(),
            }),
        }
    }
}

fn drain(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_string(&mut text);
        }
        text
    })
}

/// Formats one document (`KLF-FORMAT.md` section 6): answers its formatted
/// text, or why it was not formatted.
pub trait Formatter: Send + Sync {
    /// # Errors
    /// Returns the reason the document was not formatted.
    fn format(&self, request: &FormatRequest) -> Result<String, String>;
}

/// A document to format and how to print it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatRequest {
    pub directory: PathBuf,
    pub path: PathBuf,
    pub text: String,
    pub width: usize,
    pub tab_size: usize,
    pub insert_spaces: bool,
}

/// Formats by running `program format --stdin <path>` in a process of its
/// own, for the same reason analyses run in one.
pub struct ProcessFormatter {
    pub program: PathBuf,
}

impl Formatter for ProcessFormatter {
    fn format(&self, request: &FormatRequest) -> Result<String, String> {
        let mut command = Command::new(&self.program);
        command
            .arg("format")
            .arg("--stdin")
            .arg(&request.path)
            .arg("--width")
            .arg(request.width.to_string())
            .arg("--tab-size")
            .arg(request.tab_size.to_string());
        if !request.insert_spaces {
            command.arg("--tabs");
        }
        let mut child = command
            .current_dir(&request.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", self.program.display()))?;
        let input = request.text.clone();
        let stdin = child.stdin.take();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(input.as_bytes());
            }
        });
        let reader = drain(child.stdout.take());
        let errors = drain(child.stderr.take());
        let status = child
            .wait()
            .map_err(|error| format!("formatter: {error}"))?;
        let _ = writer.join();
        let output = reader.join().unwrap_or_default();
        let errors = errors.join().unwrap_or_default();
        if status.success() {
            Ok(output)
        } else {
            Err(errors
                .lines()
                .last()
                .unwrap_or("the formatter failed")
                .to_owned())
        }
    }
}
