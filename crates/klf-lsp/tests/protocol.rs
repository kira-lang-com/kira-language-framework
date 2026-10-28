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

#[test]
fn a_server_answers_from_the_facts_of_its_analysis() {
    let directory = std::env::temp_dir().join(format!("klf-lsp-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("area.toy");
    let uri = format!("file://{}", path.display());
    let runs = Arc::new(AtomicUsize::new(0));
    let (server, client) = Connection::memory();
    let analyzer = Fixed {
        path: path.clone(),
        runs: Arc::clone(&runs),
    };
    let thread = std::thread::spawn(move || {
        run(
            &server,
            ServerConfig {
                name: "toy".to_owned(),
                version: "0.1.0".to_owned(),
                languages: Languages::single(EditorSpec::minimal("toy")),
                analyzer: Arc::new(analyzer),
                formatter: None,
            },
        )
    });

    let mut seen = Vec::new();
    let initialized = request(
        &client,
        &mut seen,
        1,
        "initialize",
        json!({
            "capabilities": { "general": { "positionEncodings": ["utf-8", "utf-16"] } },
            "workspaceFolders": [{ "uri": format!("file://{}", directory.display()), "name": "toy" }]
        }),
    );
    assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-8");
    notify(&client, "initialized", json!({}));
    notify(
        &client,
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "toy", "version": 1, "text": TEXT } }),
    );

    let tokens = request(
        &client,
        &mut seen,
        2,
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let data: Vec<u64> = serde_json::from_value(tokens["data"].clone()).expect("token data");
    let function = 13;
    assert_eq!(
        &data[..5],
        &[0, 9, 4, function, 0],
        "`area`, classified by the analysis"
    );

    navigation(&client, &mut seen, &uri);
    assert_eq!(
        runs.load(Ordering::Relaxed),
        1,
        "one analysis serves every request of a revision"
    );
    let published = seen
        .iter()
        .find(|notification| notification.method == "textDocument/publishDiagnostics")
        .expect("diagnostics published");
    assert_eq!(published.params["diagnostics"][0]["code"], "E1");
    assert_eq!(
        published.params["diagnostics"][0]["range"]["start"],
        json!({ "line": 0, "character": 14 })
    );

    extensions(&client, &mut seen, &uri, &directory);
    assert_eq!(
        runs.load(Ordering::Relaxed),
        3,
        "a reload analyzes again, and so does the first KIR view"
    );
    assert!(
        seen.iter()
            .any(|notification| notification.method == "klf/status"),
        "klf/status is sent"
    );

    request(&client, &mut seen, 99, "shutdown", Value::Null);
    notify(&client, "exit", Value::Null);
    thread.join().expect("server thread").expect("server ran");
    let _ = std::fs::remove_dir_all(&directory);
}

/// Definition, hover, references, outline, rename and signature help.
fn navigation(client: &Connection, seen: &mut Vec<Notification>, uri: &str) {
    let position =
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 13 } });
    let definition = request(client, seen, 3, "textDocument/definition", position.clone());
    assert_eq!(
        definition[0]["range"]["start"],
        json!({ "line": 0, "character": 9 })
    );
    let hover = request(client, seen, 4, "textDocument/hover", position.clone());
    assert!(
        hover["contents"]["value"]
            .as_str()
            .expect("markdown")
            .contains("function area(r: Float) -> Float")
    );
    let references = request(
        client,
        seen,
        5,
        "textDocument/references",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 10 }, "context": { "includeDeclaration": true } }),
    );
    assert_eq!(references.as_array().map(Vec::len), Some(2));
    let symbols = request(
        client,
        seen,
        6,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(symbols[0]["name"], "area");
    let rename = request(
        client,
        seen,
        7,
        "textDocument/rename",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 10 }, "newName": "size" }),
    );
    assert_eq!(rename["changes"][uri].as_array().map(Vec::len), Some(2));
    let help = request(
        client,
        seen,
        8,
        "textDocument/signatureHelp",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 16 } }),
    );
    assert_eq!(help["signatures"][0]["parameters"][0]["label"], "r: Float");
}

/// `klf/runnables`, `klf/fileInfo` and `klf/reloadProjects` (section 17.2).
fn extensions(
    client: &Connection,
    seen: &mut Vec<Notification>,
    uri: &str,
    directory: &std::path::Path,
) {
    let document = json!({ "textDocument": { "uri": uri } });
    let runnables = request(client, seen, 20, "klf/runnables", document.clone());
    assert_eq!(runnables[0]["label"], "area");
    assert_eq!(runnables[0]["kind"], "Main");
    assert_eq!(runnables[0]["arguments"], json!(["run"]));
    assert_eq!(runnables[0]["cwd"], json!(directory));
    let outside =
        json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": 1 } });
    let none = request(client, seen, 21, "klf/runnables", outside);
    assert_eq!(none, json!([]), "no runnable holds the end of the file");
    let info = request(client, seen, 22, "klf/fileInfo", document.clone());
    assert_eq!(
        info,
        json!({
            "project": directory,
            "fileMode": "arc",
            "memory": "Counted",
            "convention": "Platform",
            "unit": 0,
        })
    );
    let reloaded = request(client, seen, 23, "klf/reloadProjects", json!({}));
    assert_eq!(reloaded, Value::Null);
    let again = request(client, seen, 24, "klf/fileInfo", document);
    assert_eq!(again["fileMode"], "arc", "answered from the new analysis");

    let at = |canonical: bool| {
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 4 },
            "canonical": canonical,
        })
    };
    for (id, canonical, stage) in [(25, false, "raw"), (27, true, "canonical")] {
        let view = request(client, seen, id, "klf/kir", at(canonical));
        let uri = view["uri"].as_str().expect("a virtual document");
        assert!(uri.starts_with("klf-virtual:"), "{uri}");
        let text = request(
            client,
            seen,
            id + 1,
            "klf/virtualFile",
            json!({ "uri": uri }),
        );
        assert_eq!(text["languageId"], "kir");
        assert!(
            text["text"]
                .as_str()
                .is_some_and(|text| text.ends_with(stage)),
            "{text}"
        );
    }
    let before = json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": 2 },
        "canonical": false,
    });
    let none = request(client, seen, 29, "klf/kir", before);
    assert_eq!(
        none,
        Value::Null,
        "no function is named before the position"
    );

    let call =
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 12 } });
    let expanded = request(client, seen, 30, "klf/expand", call);
    assert_eq!(expanded["name"], "area");
    let view = expanded["uri"].as_str().expect("a virtual document");
    let text = request(client, seen, 31, "klf/virtualFile", json!({ "uri": view }));
    assert_eq!(text["text"], "expanded(r)");
    assert_eq!(text["languageId"], "toy");
    let outside =
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } });
    let none = request(client, seen, 32, "klf/expand", outside);
    assert_eq!(none, Value::Null, "no macro was invoked there");
}
