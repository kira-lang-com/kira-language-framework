//! The cancellation rules of `KLF-EDITOR.md` sections 8.5 to 8.7, over an
//! in-memory connection with an analyzer the document text steers: `spin`
//! runs until cancelled, `trap` reports a trap in definition code, anything
//! else analyzes at once.

use klf_facts::Facts;
use klf_facts::spec::EditorSpec;
use klf_ide::{Analysis, AnalysisRequest, Analyzer, Cancellation, Interruption, Languages};
use klf_lsp::{ServerConfig, run};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const CONTENT_MODIFIED: i32 = -32801;
const REQUEST_CANCELLED: i32 = -32800;
const REQUEST_FAILED: i32 = -32803;

#[derive(Default)]
struct Steered {
    runs: AtomicUsize,
    texts: Mutex<Vec<String>>,
}

impl Analyzer for Steered {
    fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis {
        self.runs.fetch_add(1, Ordering::Relaxed);
        let text = request
            .documents
            .first()
            .map(|(_, text)| text.clone())
            .unwrap_or_default();
        self.texts.lock().expect("texts").push(text.clone());
        if text.starts_with("spin") {
            while !cancellation.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            return Analysis::interrupted(Interruption::Cancelled);
        }
        if text.starts_with("trap") {
            return Analysis::interrupted(Interruption::Trapped {
                code: "KLF-RT-0004".to_owned(),
                query: "elaborate".to_owned(),
            });
        }
        Analysis {
            facts: Facts::new(),
            ..Analysis::default()
        }
    }
}

struct Client {
    connection: Connection,
    seen: Vec<Notification>,
    server: Option<JoinHandle<Result<(), String>>>,
    directory: PathBuf,
    uri: String,
}

impl Client {
    fn start(name: &str, analyzer: Arc<Steered>) -> Self {
        let directory =
            std::env::temp_dir().join(format!("klf-lsp-cancel-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        let uri = format!("file://{}", directory.join("main.toy").display());
        let (server, connection) = Connection::memory();
        let server = std::thread::spawn(move || {
            run(
                &server,
                ServerConfig {
                    name: "toy".to_owned(),
                    version: "0.1.0".to_owned(),
                    languages: Languages::single(EditorSpec::minimal("toy")),
                    analyzer,
                    formatter: None,
                },
            )
        });
        let mut client = Self {
            connection,
            seen: Vec::new(),
            server: Some(server),
            directory,
            uri,
        };
        let folder = format!("file://{}", client.directory.display());
        client.send(
            1,
            "initialize",
            json!({ "capabilities": {}, "workspaceFolders": [{ "uri": folder, "name": "toy" }] }),
        );
        client.response(1);
        client.notify("initialized", json!({}));
        client
    }

    fn send(&self, id: i32, method: &str, params: Value) {
        let request = Request::new(RequestId::from(id), method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Request(request))
            .expect("send");
    }

    fn notify(&self, method: &str, params: Value) {
        let notification = Notification::new(method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Notification(notification))
            .expect("send");
    }

    fn open(&self, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": self.uri, "languageId": "toy", "version": 1, "text": text } }),
        );
    }

    fn change(&self, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": self.uri, "version": version }, "contentChanges": [{ "text": text }] }),
        );
    }

    fn hover(&self, id: i32) {
        self.send(
            id,
            "textDocument/hover",
            json!({ "textDocument": { "uri": self.uri }, "position": { "line": 0, "character": 0 } }),
        );
    }

    fn tokens(&self, id: i32) {
        self.send(
            id,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": self.uri } }),
        );
    }

    /// Waits for the response to `id`, keeping the notifications meanwhile.
    fn response(&mut self, id: i32) -> Response {
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("the server answers");
            match message {
                Message::Response(response) if response.id == RequestId::from(id) => {
                    return response;
                }
                Message::Notification(notification) => self.seen.push(notification),
                _ => {}
            }
        }
    }

    /// Waits for a notification `method` whose message contains `text`.
    fn wait_for(&mut self, method: &str, text: &str) -> Notification {
        let matches = |notification: &Notification| {
            notification.method == method && notification.params.to_string().contains(text)
        };
        if let Some(found) = self.seen.iter().find(|n| matches(n)) {
            return found.clone();
        }
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("the server notifies");
            if let Message::Notification(notification) = message {
                self.seen.push(notification.clone());
                if matches(&notification) {
                    return notification;
                }
            }
        }
    }

    fn count(&self, method: &str, text: &str) -> usize {
        self.seen
            .iter()
            .filter(|n| n.method == method && n.params.to_string().contains(text))
            .count()
    }

    fn stop(mut self) {
        self.send(99, "shutdown", Value::Null);
        self.response(99);
        self.notify("exit", Value::Null);
        if let Some(server) = self.server.take() {
            server.join().expect("server thread").expect("server ran");
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn error_code(response: &Response) -> Option<i32> {
    response.error.as_ref().map(|error| error.code)
}

#[test]
fn a_write_cancels_the_running_analysis_within_budget() {
    let analyzer = Arc::new(Steered::default());
    let mut client = Client::start("write", Arc::clone(&analyzer));
    client.open("spin");
    client.tokens(2);
    client.hover(3);
    // Let the analysis of the first revision start before the write.
    while analyzer.runs.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    client.change(2, "plain");

    let tokens = client.response(2);
    assert_eq!(
        error_code(&tokens),
        Some(CONTENT_MODIFIED),
        "semantic tokens are asked for again"
    );
    let hover = client.response(3);
    assert_eq!(
        error_code(&hover),
        None,
        "hover is retried at the new revision"
    );

    let log = client.wait_for("window/logMessage", "cancelled in");
    let message = log.params["message"].as_str().expect("message");
    let released: u64 = message
        .split("cancelled in ")
        .nth(1)
        .and_then(|rest| rest.split(" ms").next())
        .and_then(|ms| ms.parse().ok())
        .expect("the release time is logged");
    assert!(released <= 20, "released after {released} ms: {message}");
    assert_eq!(
        analyzer
            .texts
            .lock()
            .expect("texts")
            .last()
            .map(String::as_str),
        Some("plain")
    );
    client.stop();
}

#[test]
fn cancel_request_answers_request_cancelled() {
    let analyzer = Arc::new(Steered::default());
    let mut client = Client::start("cancel-request", Arc::clone(&analyzer));
    client.open("spin");
    client.hover(2);
    client.notify("$/cancelRequest", json!({ "id": 2 }));
    let response = client.response(2);
    assert_eq!(error_code(&response), Some(REQUEST_CANCELLED));
    // The open document still needs its analysis; a write releases it.
    client.change(2, "plain");
    client.hover(3);
    assert_eq!(error_code(&client.response(3)), None);
    client.stop();
}

#[test]
fn a_trap_fails_the_request_and_the_server_keeps_answering() {
    let analyzer = Arc::new(Steered::default());
    let mut client = Client::start("trap", Arc::clone(&analyzer));
    client.open("trap");
    client.hover(2);
    let failed = client.response(2);
    assert_eq!(error_code(&failed), Some(REQUEST_FAILED));
    let message = failed.error.map(|error| error.message).unwrap_or_default();
    assert!(message.contains("KLF-LSP-9001"), "{message}");
    assert!(
        message.contains("KLF-RT-0004") && message.contains("elaborate"),
        "{message}"
    );

    client.change(2, "trap again");
    client.hover(3);
    assert_eq!(error_code(&client.response(3)), Some(REQUEST_FAILED));
    assert_eq!(
        client.count("window/showMessage", "KLF-LSP-9001"),
        1,
        "shown once per query key"
    );
    assert_eq!(
        client.count("window/logMessage", "KLF-LSP-9001"),
        2,
        "logged every time"
    );

    client.change(3, "plain");
    client.hover(4);
    assert_eq!(
        error_code(&client.response(4)),
        None,
        "the server keeps answering"
    );
    client.stop();
}

#[test]
fn diagnostics_wait_for_the_debounce() {
    let analyzer = Arc::new(Steered::default());
    let mut client = Client::start("debounce", Arc::clone(&analyzer));
    client.open("plain 1");
    for version in 2..6 {
        client.change(version, &format!("plain {version}"));
    }
    client.wait_for("textDocument/publishDiagnostics", "");
    assert_eq!(
        analyzer.runs.load(Ordering::Relaxed),
        1,
        "changes within the debounce are analyzed once"
    );
    assert_eq!(
        analyzer.texts.lock().expect("texts").as_slice(),
        ["plain 5"]
    );
    client.stop();
}
