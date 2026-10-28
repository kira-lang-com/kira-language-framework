//! The cancellation suite of `KLF-EDITOR.md` section 24.1 against a real
//! native frontend: `<driver> lsp` analyzes in its own process, and a write
//! during a long loop, a deep recursion or a loop that allocates, in every
//! phase, releases the analysis within budget with the heap balanced. A trap
//! fails the request with `KLF-LSP-9001` and the server keeps answering.
//!
//! The definition, `tests/spin`, misbehaves as its source tells it to.

use klf_driver::{build_frontend, toolchain_available};
use klf_services::Session;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How long a cancelled analysis may take to release its lane (section 24.1).
/// The median of the runs is held to it: a machine short of memory or cores
/// stalls a single run on page faults or the scheduler, not on a poll.
const BUDGET_MS: u64 = 20;

/// What any one run may take. An analysis that misses its polls is never
/// released at all.
const CEILING_MS: u64 = 1000;

/// How long a misbehaving phase runs before the write that cancels it.
const RUNNING: Duration = Duration::from_millis(150);

struct Server {
    child: Child,
    input: ChildStdin,
    messages: Receiver<String>,
    seen: Vec<String>,
    uri: String,
    version: i32,
}

impl Server {
    fn start(binary: &Path, directory: &Path) -> Self {
        let mut child = Command::new(binary)
            .arg("lsp")
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the server starts");
        let input = child.stdin.take().expect("stdin");
        let output = child.stdout.take().expect("stdout");
        let (sender, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length: ") {
                        length = value.parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if sender
                    .send(String::from_utf8_lossy(&body).into_owned())
                    .is_err()
                {
                    return;
                }
            }
        });
        let uri = format!("file://{}", directory.join("main.spin").display());
        let mut server = Self {
            child,
            input,
            messages,
            seen: Vec::new(),
            uri,
            version: 1,
        };
        let folder = format!("file://{}", directory.display());
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{}},"workspaceFolders":[{{"uri":"{folder}","name":"spin"}}]}}}}"#
        ));
        server.response(1);
        server.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        server
    }

    fn send(&mut self, body: &str) {
        write!(self.input, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
        self.input.flush().expect("flush");
    }

    fn open(&mut self, text: &str) {
        let uri = self.uri.clone();
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"spin","version":1,"text":"{text}"}}}}}}"#
        ));
    }

    fn change(&mut self, text: &str) {
        self.version += 1;
        let (uri, version) = (self.uri.clone(), self.version);
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{uri}","version":{version}}},"contentChanges":[{{"text":"{text}"}}]}}}}"#
        ));
    }

    fn hover(&mut self, id: i32) {
        let uri = self.uri.clone();
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":0}}}}}}"#
        ));
    }

    fn next(&mut self) -> String {
        let message = self
            .messages
            .recv_timeout(Duration::from_secs(30))
            .expect("the server answers");
        self.seen.push(message.clone());
        message
    }

    /// The response to request `id`.
    fn response(&mut self, id: i32) -> String {
        let answered = [
            format!(r#""id":{id},"result""#),
            format!(r#""id":{id},"error""#),
        ];
        loop {
            let message = self.next();
            if answered
                .iter()
                .any(|marker| message.contains(marker.as_str()))
            {
                return message;
            }
        }
    }

    /// The first message, seen or still to come, that contains `text`.
    fn wait_for(&mut self, text: &str) -> String {
        if let Some(found) = self.seen.iter().find(|message| message.contains(text)) {
            return found.clone();
        }
        loop {
            let message = self.next();
            if message.contains(text) {
                return message;
            }
        }
    }

    fn count(&self, text: &str) -> usize {
        self.seen
            .iter()
            .filter(|message| message.contains(text))
            .count()
    }

    fn stop(mut self) {
        self.send(r#"{"jsonrpc":"2.0","id":999,"method":"shutdown"}"#);
        self.response(999);
        self.send(r#"{"jsonrpc":"2.0","method":"exit"}"#);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                assert!(status.success(), "the server exits cleanly: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "the server exits");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn built() -> Option<(PathBuf, PathBuf)> {
    if !toolchain_available() {
        eprintln!("skipping: no provisioned clang");
        return None;
    }
    if !cfg!(unix) {
        eprintln!("skipping: definition code unwinds only on ELF and Mach-O hosts");
        return None;
    }
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/spin");
    let directory = std::env::temp_dir().join(format!("klf-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp directory");
    let binary = directory.join("spin");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
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
    std::fs::write(directory.join("main.spin"), "ok").expect("input written");
    Some((binary, directory))
}

/// The milliseconds and the heap note of the `n`th cancellation logged.
fn cancellation(server: &mut Server, n: usize) -> (u64, String) {
    while server.count("cancelled in") < n {
        server.next();
    }
    let message = server
        .seen
        .iter()
        .filter(|message| message.contains("cancelled in"))
        .nth(n - 1)
        .cloned()
        .expect("logged");
    let after = message
        .split("cancelled in ")
        .nth(1)
        .expect("released")
        .to_owned();
    let milliseconds = after
        .split(" ms")
        .next()
        .and_then(|ms| ms.parse().ok())
        .expect("milliseconds");
    let note = after
        .split("; ")
        .nth(1)
        .unwrap_or("")
        .split('"')
        .next()
        .unwrap_or("")
        .to_owned();
    (milliseconds, note)
}

#[test]
fn writes_cancel_definition_code_and_traps_do_not_end_the_server() {
    let Some((binary, directory)) = built() else {
        return;
    };
    let mut server = Server::start(&binary, &directory);
    server.open("ok");
    server.hover(2);
    let answered = server.response(2);
    assert!(!answered.contains(r#""error""#), "{answered}");

    let mut id = 10;
    let mut cancelled = 0;
    let mut releases = Vec::new();
    for what in ["s", "r", "a"] {
        for phase in ["1", "2", "3"] {
            server.change(&format!("{what}{phase}"));
            id += 1;
            server.hover(id);
            std::thread::sleep(RUNNING);
            server.change("ok");
            let answered = server.response(id);
            assert!(
                !answered.contains(r#""error""#),
                "{what}{phase}: hover is retried: {answered}"
            );
            cancelled += 1;
            let (milliseconds, note) = cancellation(&mut server, cancelled);
            assert!(
                milliseconds <= CEILING_MS,
                "{what}{phase}: released after {milliseconds} ms"
            );
            assert_eq!(note, "the heap is balanced", "{what}{phase}");
            releases.push(milliseconds);
        }
    }
    releases.sort_unstable();
    let median = releases[releases.len() / 2];
    assert!(
        median <= BUDGET_MS,
        "released after {releases:?} ms, median {median}"
    );

    for phase in ["1", "2", "3"] {
        server.change(&format!("t{phase}"));
        id += 1;
        server.hover(id);
        let failed = server.response(id);
        assert!(
            failed.contains("-32803"),
            "t{phase}: RequestFailed: {failed}"
        );
        assert!(failed.contains("KLF-LSP-9001"), "t{phase}: {failed}");
    }
    server.change("t1");
    id += 1;
    server.hover(id);
    server.response(id);
    assert_eq!(
        server.count("window/showMessage"),
        3,
        "shown once per query key: parse, elaborate and lower"
    );

    server.change("ok");
    id += 1;
    server.hover(id);
    let answered = server.response(id);
    assert!(
        !answered.contains(r#""error""#),
        "the server keeps answering: {answered}"
    );
    server.wait_for("publishDiagnostics");
    server.stop();
    let _ = std::fs::remove_dir_all(&directory);
}
