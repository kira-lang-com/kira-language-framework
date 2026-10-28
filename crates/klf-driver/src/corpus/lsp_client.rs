//! A minimal language server client for the editor corpus: it starts
//! `<driver> lsp`, speaks JSON-RPC over the child's standard streams, and
//! keeps the diagnostics the server publishes.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// How long one answer may take: an analysis starts a worker process and
/// runs the definition's phases.
const PATIENCE: Duration = Duration::from_mins(2);

pub(crate) struct Client {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
    /// The latest diagnostics published for each document URI.
    pub(crate) diagnostics: HashMap<String, Vec<Value>>,
    /// The semantic token types and modifiers, in the server's legend order.
    pub(crate) legend: (Vec<String>, Vec<String>),
}

pub(crate) fn uri(path: &Path) -> String {
    let text = path
        .to_string_lossy()
        .replace('%', "%25")
        .replace(' ', "%20");
    format!("file://{text}")
}

impl Client {
    /// Starts `driver lsp` in `directory` and initializes it with UTF-8
    /// positions, so every position is a byte column.
    pub(crate) fn start(driver: &Path, directory: &Path) -> Result<Self, String> {
        let mut child = Command::new(driver)
            .arg("lsp")
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot start `{} lsp`: {error}", driver.display()))?;
        let input = child.stdin.take().ok_or("no server input")?;
        let output = child.stdout.take().ok_or("no server output")?;
        let (sender, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Some(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            messages,
            next_id: 0,
            diagnostics: HashMap::new(),
            legend: (Vec::new(), Vec::new()),
        };
        let root = uri(directory);
        let result = client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root,
                "workspaceFolders": [{ "uri": root, "name": "case" }],
                "capabilities": {
                    "general": { "positionEncodings": ["utf-8"] },
                    "textDocument": { "semanticTokens": {
                        "requests": { "full": true },
                        "tokenTypes": [],
                        "tokenModifiers": [],
                        "formats": ["relative"],
                        "multilineTokenSupport": false
                    } }
                }
            }),
        )?;
        let capabilities = &result["capabilities"];
        if capabilities["positionEncoding"] != "utf-8" {
            return Err(format!(
                "the server chose the position encoding {}, not utf-8",
                capabilities["positionEncoding"]
            ));
        }
        let names = |key: &str| -> Vec<String> {
            capabilities["semanticTokensProvider"]["legend"][key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        client.legend = (names("tokenTypes"), names("tokenModifiers"));
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        let body = message.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{body}", body.len())
            .and_then(|()| self.input.flush())
            .map_err(|error| format!("the server stopped reading: {error}"))
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let mut message = json!({ "jsonrpc": "2.0", "method": method });
        message["params"] = params;
        self.send(&message)
    }

    /// Sends a request and answers its result, handling what the server
    /// sends meanwhile.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        message["params"] = params;
        self.send(&message)?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let message = self.receive(deadline, method)?;
            if message.get("method").is_none() && message["id"] == id {
                if let Some(error) = message.get("error") {
                    return Err(format!("{method} failed: {}", error["message"]));
                }
                return Ok(message["result"].clone());
            }
        }
    }

    /// Waits until the server has published diagnostics for every URI of
    /// `uris`.
    pub(crate) fn await_diagnostics(&mut self, uris: &[String]) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        while !uris.iter().all(|uri| self.diagnostics.contains_key(uri)) {
            self.receive(deadline, "textDocument/publishDiagnostics")?;
        }
        Ok(())
    }

    /// The next message, after answering a server request and keeping
    /// published diagnostics.
    fn receive(&mut self, deadline: Instant, waiting: &str) -> Result<Value, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        let message = match self.messages.recv_timeout(left) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!("no answer to {waiting} in {}s", PATIENCE.as_secs()));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(format!("the server ended while answering {waiting}"));
            }
        };
        match message["method"].as_str() {
            Some("textDocument/publishDiagnostics") => {
                let params = &message["params"];
                if let Some(uri) = params["uri"].as_str() {
                    let list = params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    self.diagnostics.insert(uri.to_owned(), list);
                }
            }
            Some(_) if message.get("id").is_some() => {
                let id = message["id"].clone();
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": null }))?;
            }
            _ => {}
        }
        Ok(message)
    }

    pub(crate) fn shut_down(mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}
