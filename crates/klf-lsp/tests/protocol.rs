//! The server over an in-memory connection, analyzing with a fixed analyzer.

use klf_core::FileId;
use klf_diag::Severity;
use klf_facts::spec::EditorSpec;
use klf_facts::{
    Entity, Facts, ReferenceRole, Runnable, RunnableKind, Symbol, SymbolKind, TokenClass,
};
use klf_ide::{
    Analysis, AnalysisRequest, Analyzer, Cancellation, FileInfo, IdeDiagnostic, KirFunction,
    Languages,
};
use klf_lsp::{ServerConfig, run};
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "protocol/navigation.rs"]
mod navigation;

const TEXT: &str = "function area(r) {\n    return area(r)\n}\n";

struct Fixed {
    path: PathBuf,
    runs: Arc<AtomicUsize>,
}

impl Analyzer for Fixed {
    fn analyze(&self, request: &AnalysisRequest, _: &Cancellation) -> Analysis {
        self.runs.fetch_add(1, Ordering::Relaxed);
        let text = &request
            .documents
            .iter()
            .find(|(path, _)| *path == self.path)
            .expect("document sent")
            .1;
        assert_eq!(text, TEXT);
        let file = FileId::from_u32(0);
        let mut facts = Facts::new();
        facts.classify(file, 9, 13, TokenClass::Function, 0);
        facts.declare_symbol(
            Entity(1),
            Symbol {
                name: "area".to_owned(),
                kind: Some(SymbolKind::Function),
                file: Some(file),
                start: 0,
                end: 40,
                selection_start: 9,
                selection_end: 13,
                signature: "function area(r: Float) -> Float".to_owned(),
                parameters: vec!["r: Float".to_owned()],
                ..Symbol::default()
            },
        );
        facts.reference(file, 30, 34, Entity(1), ReferenceRole::Call);
        facts.runnable(
            file,
            Runnable {
                label: "area".to_owned(),
                kind: RunnableKind::Main,
                start: 0,
                end: 18,
                arguments: vec!["run".to_owned()],
            },
        );
        Analysis {
            files: vec![(file, self.path.clone())],
            diagnostics: vec![IdeDiagnostic {
                file,
                start: 14,
                end: 15,
                severity: Severity::Error,
                code: "E1".to_owned(),
                message: "untyped parameter".to_owned(),
                source: None,
            }],
            facts,
            failure: None,
            interrupted: None,
            file_info: vec![FileInfo {
                file,
                mode: "arc".to_owned(),
                memory: "Counted".to_owned(),
                convention: "Platform".to_owned(),
                unit: Some(0),
            }],
            kir: if request.kir {
                vec![KirFunction {
                    file,
                    start: 9,
                    end: 13,
                    name: "area".to_owned(),
                    raw: "func module @area() -> i64 engine(any) raw".to_owned(),
                    canonical: "func module @area() -> i64 engine(any) canonical".to_owned(),
                }]
            } else {
                Vec::new()
            },
            expansions: vec![klf_ide::Expansion {
                origin: file,
                start: 30,
                end: 34,
                name: "area".to_owned(),
                file: FileId::from_u32(1),
                text: "expanded(r)".to_owned(),
            }],
            shader_outputs: Vec::new(),
        }
    }
}

/// Sends a request and waits for its response, keeping the notifications
/// that arrive meanwhile.
fn request(
    client: &Connection,
    seen: &mut Vec<Notification>,
    id: i32,
    method: &str,
    params: Value,
) -> Value {
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(id),
            method.to_owned(),
            params,
        )))
        .expect("send");
    loop {
        match client.receiver.recv().expect("server answers") {
            Message::Response(response) if response.id == RequestId::from(id) => {
                assert!(response.error.is_none(), "{method}: {:?}", response.error);
                return response.result.unwrap_or(Value::Null);
            }
            Message::Notification(notification) => seen.push(notification),
            _ => {}
        }
    }
}

fn notify(client: &Connection, method: &str, params: Value) {
    client
        .sender
        .send(Message::Notification(Notification::new(
            method.to_owned(),
            params,
        )))
        .expect("send");
}

fn file_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

#[test]
fn project_watch_is_registered_and_change_resolves_again() {
    let directory = std::env::temp_dir();
    let path = directory.join("watched.toy");
    let uri = file_uri(&path);
    let calls = Arc::new(AtomicUsize::new(0));
    let resolved = Arc::clone(&calls);
    let root = directory.clone();
    let (server, client) = Connection::memory();
    let thread = std::thread::spawn(move || {
        run(
            &server,
            ServerConfig {
                name: "toy".into(),
                version: "1".into(),
                languages: Languages::single(EditorSpec::minimal("toy")),
                analyzer: Arc::new(Fixed {
                    path,
                    runs: Arc::new(AtomicUsize::new(0)),
                }),
                formatter: None,
                project_resolver: Some(Arc::new(move |_| {
                    resolved.fetch_add(1, Ordering::Relaxed);
                    Ok(klf_ide::ProjectResolution {
                        project: Some(klf_ide::Project {
                            name: "toy".into(),
                            root: root.clone(),
                            arguments: vec!["watched.toy".into()],
                            loose: false,
                            watch: vec!["manifest.json".into()],
                            output: None,
                        }),
                        ..Default::default()
                    })
                })),
            },
        )
    });
    let mut seen = Vec::new();
    request(
        &client,
        &mut seen,
        1,
        "initialize",
        json!({
            "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }
        }),
    );
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({
            "textDocument": { "uri": uri, "languageId": "toy", "version": 1, "text": TEXT }
        }),
    );
    loop {
        let message = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("watch registration");
        if let Message::Request(request) = message {
            assert_eq!(request.method, "client/registerCapability");
            assert!(
                request.params["registrations"][0]["registerOptions"]["watchers"][0]["globPattern"]
                    .as_str()
                    .unwrap()
                    .ends_with("manifest.json")
            );
            break;
        }
    }
    notify(
        &client,
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{
        "uri": file_uri(&directory.join("manifest.json")), "type": 2
    }] }),
    );
    request(&client, &mut seen, 99, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    thread.join().expect("server thread").expect("server ran");
    assert!(calls.load(Ordering::Relaxed) >= 2);
}

#[test]
fn project_failure_is_published_with_loose_file_analysis() {
    let path = std::env::temp_dir().join("broken-project.toy");
    let uri = file_uri(&path);
    let (server, client) = Connection::memory();
    let thread = std::thread::spawn(move || {
        run(
            &server,
            ServerConfig {
                name: "toy".into(),
                version: "1".into(),
                languages: Languages::single(EditorSpec::minimal("toy")),
                analyzer: Arc::new(Fixed {
                    path,
                    runs: Arc::new(AtomicUsize::new(0)),
                }),
                formatter: None,
                project_resolver: Some(Arc::new(|_| {
                    Err(klf_ide::ProjectError {
                        message: "invalid manifest".into(),
                        code: Some(42),
                        span: Some((9, 13)),
                        path: None,
                    })
                })),
            },
        )
    });
    let mut seen = Vec::new();
    request(
        &client,
        &mut seen,
        1,
        "initialize",
        json!({ "capabilities": {} }),
    );
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({
            "textDocument": { "uri": uri, "languageId": "toy", "version": 1, "text": TEXT }
        }),
    );
    loop {
        let message = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("diagnostics");
        if let Message::Notification(notification) = message
            && notification.method == "textDocument/publishDiagnostics"
        {
            let diagnostics = notification.params["diagnostics"].as_array().unwrap();
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["message"] == "invalid manifest")
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "42"
                        && diagnostic["range"]["start"]["character"] == 9
                        && diagnostic["range"]["end"]["character"] == 13)
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "E1")
            );
            break;
        }
    }
    request(&client, &mut seen, 99, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    thread.join().expect("server thread").expect("server ran");
}

#[test]
fn project_failure_is_published_on_the_manifest() {
    let directory = std::env::temp_dir().join(format!("klf-project-error-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("input.toy");
    let manifest = directory.join("project.json");
    std::fs::write(&manifest, "first line\ninvalid manifest\n").unwrap();
    let manifest_uri = file_uri(&manifest);
    let uri = file_uri(&path);
    let (server, client) = Connection::memory();
    let thread = std::thread::spawn(move || {
        run(
            &server,
            ServerConfig {
                name: "toy".into(),
                version: "1".into(),
                languages: Languages::single(EditorSpec::minimal("toy")),
                analyzer: Arc::new(Fixed {
                    path,
                    runs: Arc::new(AtomicUsize::new(0)),
                }),
                formatter: None,
                project_resolver: Some(Arc::new(move |_| {
                    Err(klf_ide::ProjectError {
                        message: "invalid manifest".into(),
                        code: Some(42),
                        span: Some((11, 18)),
                        path: Some(manifest.clone()),
                    })
                })),
            },
        )
    });
    let mut seen = Vec::new();
    request(
        &client,
        &mut seen,
        1,
        "initialize",
        json!({
            "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }
        }),
    );
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({
            "textDocument": { "uri": uri, "languageId": "toy", "version": 1, "text": TEXT }
        }),
    );
    let mut watched = false;
    let mut published = false;
    while !watched || !published {
        let message = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("diagnostics");
        if let Message::Request(request) = &message {
            assert_eq!(request.method, "client/registerCapability");
            assert!(
                request.params["registrations"][0]["registerOptions"]["watchers"][0]["globPattern"]
                    .as_str()
                    .unwrap()
                    .ends_with("project.json")
            );
            watched = true;
        }
        if let Message::Notification(notification) = message
            && notification.method == "textDocument/publishDiagnostics"
            && notification.params["uri"] == manifest_uri
        {
            let diagnostic = &notification.params["diagnostics"][0];
            assert_eq!(diagnostic["message"], "invalid manifest");
            assert_eq!(diagnostic["code"], "42");
            assert_eq!(
                diagnostic["range"]["start"],
                json!({"line": 1, "character": 0})
            );
            assert_eq!(
                diagnostic["range"]["end"],
                json!({"line": 1, "character": 7})
            );
            published = true;
        }
    }
    request(&client, &mut seen, 99, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    thread.join().unwrap().unwrap();
}

#[test]
fn project_warning_is_published_without_discarding_the_project() {
    struct ExpectProject {
        fixed: Fixed,
        argument: String,
    }
    impl Analyzer for ExpectProject {
        fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis {
            assert_eq!(request.arguments, [self.argument.clone()]);
            self.fixed.analyze(request, cancellation)
        }
    }
    let directory =
        std::env::temp_dir().join(format!("klf-project-warning-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("input.toy");
    let uri = file_uri(&path);
    let root = directory.clone();
    let argument = root.to_string_lossy().into_owned();
    let (server, client) = Connection::memory();
    let thread = std::thread::spawn(move || {
        run(
            &server,
            ServerConfig {
                name: "toy".into(),
                version: "1".into(),
                languages: Languages::single(EditorSpec::minimal("toy")),
                analyzer: Arc::new(ExpectProject {
                    fixed: Fixed {
                        path,
                        runs: Arc::new(AtomicUsize::new(0)),
                    },
                    argument,
                }),
                formatter: None,
                project_resolver: Some(Arc::new(move |_| {
                    Ok(klf_ide::ProjectResolution {
                        project: Some(klf_ide::Project {
                            name: "resolved".into(),
                            root: root.clone(),
                            arguments: vec![root.to_string_lossy().into_owned()],
                            loose: false,
                            watch: vec![],
                            output: None,
                        }),
                        diagnostics: vec![klf_ide::ProjectDiagnostic {
                            message: "project warning".into(),
                            code: Some(43),
                            span: Some((9, 13)),
                            path: None,
                            severity: Severity::Warning,
                        }],
                        failed: false,
                    })
                })),
            },
        )
    });
    let mut seen = Vec::new();
    request(
        &client,
        &mut seen,
        1,
        "initialize",
        json!({ "capabilities": {} }),
    );
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({
            "textDocument": { "uri": uri, "languageId": "toy", "version": 1, "text": TEXT }
        }),
    );
    loop {
        let message = client
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("diagnostics");
        if let Message::Notification(notification) = message
            && notification.method == "textDocument/publishDiagnostics"
            && notification.params["uri"] == uri
        {
            let diagnostics = notification.params["diagnostics"].as_array().unwrap();
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "43"
                        && diagnostic["severity"] == 2
                        && diagnostic["message"] == "project warning")
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "E1")
            );
            break;
        }
    }
    request(&client, &mut seen, 99, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    thread.join().unwrap().unwrap();
}
