//! The `klf/` protocol extensions of section 17: a virtual document the
//! client opens by URI, and the shader output views of section 14.5.
//!
//! A `klf-virtual:` URI names one view of one project's latest analysis. The
//! server keeps their content per revision and tells the client which URIs
//! moved when a revision lands.

use crate::handlers::reply;
use crate::server::Server;
use klf_ide::{Analysis, Project};
use lsp_server::{ErrorCode, Request, Response};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The scheme of a virtual document's URI.
const SCHEME: &str = "klf-virtual";

/// What a view shows, so that its text follows each new revision.
#[derive(Clone)]
pub(crate) enum Source {
    /// One output of a shader unit (section 14.5).
    Shader {
        file: PathBuf,
        target: String,
        kind: String,
    },
    /// One function's KIR (section 13.7).
    Kir {
        file: PathBuf,
        function: String,
        canonical: bool,
    },
    /// What the macro `name` invoked at `start` of `origin` expanded to
    /// (section 13.6), in the language of `origin`.
    Expansion {
        origin: PathBuf,
        start: u32,
        name: String,
        language_id: String,
    },
}

impl Source {
    /// The language id and text of the view in `analysis`.
    fn read(&self, analysis: &Analysis) -> Option<(String, String)> {
        match self {
            Self::Shader { file, target, kind } => {
                let file = analysis.file(file)?;
                analysis
                    .shader_outputs
                    .iter()
                    .find(|output| {
                        output.file == file && output.target == *target && output.kind == *kind
                    })
                    .map(|output| (output.language_id.clone(), output.text.clone()))
            }
            Self::Kir {
                file,
                function,
                canonical,
            } => {
                let file = analysis.file(file)?;
                analysis
                    .kir
                    .iter()
                    .find(|kir| kir.file == file && kir.name == *function)
                    .map(|kir| {
                        let text = if *canonical { &kir.canonical } else { &kir.raw };
                        ("kir".to_owned(), text.clone())
                    })
            }
            Self::Expansion {
                origin,
                start,
                name,
                language_id,
            } => {
                let origin = analysis.file(origin)?;
                analysis
                    .expansions
                    .iter()
                    .find(|expansion| {
                        expansion.origin == origin
                            && expansion.start == *start
                            && expansion.name == *name
                    })
                    .map(|expansion| (language_id.clone(), expansion.text.clone()))
            }
        }
    }
}

struct View {
    project: Project,
    source: Source,
    language_id: String,
    text: String,
}

/// The views the client opened, by URI.
#[derive(Default)]
pub(crate) struct Views {
    views: RefCell<BTreeMap<String, View>>,
}

impl Views {
    /// A view's text and language, by URI.
    pub(crate) fn get(&self, uri: &str) -> Option<(String, String)> {
        self.views
            .borrow()
            .get(uri)
            .map(|view| (view.text.clone(), view.language_id.clone()))
    }

    /// Records the view `source` names in `analysis`, and returns its URI
    /// and language id; `None` when the analysis holds no such view.
    pub(crate) fn open(
        &self,
        project: &Project,
        name: &str,
        source: Source,
        analysis: &Analysis,
    ) -> Option<(String, String)> {
        let (language_id, text) = source.read(analysis)?;
        let uri = format!("{SCHEME}:{}/{name}", project.root.display());
        self.views.borrow_mut().insert(
            uri.clone(),
            View {
                project: project.clone(),
                source,
                language_id: language_id.clone(),
                text,
            },
        );
        Some((uri, language_id))
    }

    /// Reads every view of `project` again from its new analysis, and
    /// returns their URIs. A view the analysis no longer holds keeps its
    /// last text.
    pub(crate) fn refresh(&self, project: &Project, analysis: &Analysis) -> Vec<String> {
        let mut views = self.views.borrow_mut();
        let mut moved = Vec::new();
        for (uri, view) in views
            .iter_mut()
            .filter(|(_, view)| view.project == *project)
        {
            if let Some((language_id, text)) = view.source.read(analysis) {
                view.language_id = language_id;
                view.text = text;
            }
            moved.push(uri.clone());
        }
        moved
    }
}

/// The `klf/` methods. None of them waits on an analysis: a view is looked up
/// by URI, and a request for one that the latest analysis does not hold says
/// so rather than waiting.
pub(crate) fn answer(server: &mut Server<'_>, request: &Request) -> Option<Response> {
    let method = request.method.as_str();
    if !method.starts_with("klf/") {
        return None;
    }
    Some(match method {
        "klf/virtualFile" => virtual_file(server, request),
        "klf/shaderOutput" => shader_output(server, request),
        "klf/status" => reply(request, status_of(server)),
        "klf/reloadProjects" => {
            server.reload_projects();
            reply(request, serde_json::Value::Null)
        }
        // These are about a document, so they wait for its analysis.
        "klf/runnables" | "klf/fileInfo" | "klf/kir" | "klf/expand" => return None,
        _ => crate::server::unhandled(request.id.clone(), method),
    })
}

/// `klf/virtualFile`: the text and language of a `klf-virtual:` document.
fn virtual_file(server: &Server<'_>, request: &Request) -> Response {
    let Some(uri) = text(request, "uri") else {
        return invalid(request, "klf/virtualFile needs a `uri`");
    };
    match server.views.get(&uri) {
        Some((text, language_id)) => reply(
            request,
            serde_json::json!({ "text": text, "languageId": language_id }),
        ),
        None => invalid(request, &format!("no virtual document at {uri}")),
    }
}

/// `klf/shaderOutput`: the URI and language id of one view of a shader's
/// output. Registering the view is what lets the client open it by URI.
fn shader_output(server: &Server<'_>, request: &Request) -> Response {
    let (Some(document), Some(target), Some(kind)) = (
        nested_text(request, "textDocument", "uri"),
        text(request, "target"),
        text(request, "kind"),
    ) else {
        return invalid(
            request,
            "klf/shaderOutput needs a textDocument, target and kind",
        );
    };
    let Some(project) = server.project_naming(&document) else {
        return invalid(request, &format!("{document} is in no analyzed project"));
    };
    let Some(path) = document
        .parse()
        .ok()
        .and_then(|uri| crate::convert::path(&uri))
    else {
        return invalid(request, &format!("{document} is not a file URI"));
    };
    let Some(analyzed) = server.latest(&project) else {
        return invalid(request, &format!("{document} has no analysis yet"));
    };
    let analysis = &analyzed.analysis;
    // The view belongs to the document's own file, so two shaders in one
    // file each get their own.
    let name = analysis.file(&path).and_then(|file| {
        analysis
            .shader_outputs
            .iter()
            .find(|output| output.file == file && output.target == target && output.kind == kind)
            .map(|output| output.name.clone())
    });
    let source = Source::Shader {
        file: path,
        target: target.clone(),
        kind: kind.clone(),
    };
    match name.and_then(|name| server.views.open(&project, &name, source, analysis)) {
        Some((uri, language_id)) => reply(
            request,
            serde_json::json!({ "uri": uri, "languageId": language_id }),
        ),
        None => invalid(
            request,
            &format!("{document} produced no {target} {kind} view"),
        ),
    }
}

/// `klf/status`: what the server last saw. It answers the request and is
/// the notification sent whenever it changes.
pub(crate) fn status_of(server: &Server<'_>) -> serde_json::Value {
    let projects = server.project_count();
    let warning = server.any_failure();
    serde_json::json!({
        "health": if warning { "warning" } else { "ok" },
        "message": format!("{} serving {projects} projects", server.source()),
        "indexing": server.any_indexing(),
    })
}

fn text(request: &Request, name: &str) -> Option<String> {
    request.params.get(name)?.as_str().map(str::to_owned)
}

fn nested_text(request: &Request, outer: &str, inner: &str) -> Option<String> {
    request
        .params
        .get(outer)?
        .get(inner)?
        .as_str()
        .map(str::to_owned)
}

fn invalid(request: &Request, message: &str) -> Response {
    Response::new_err(
        request.id.clone(),
        ErrorCode::InvalidParams as i32,
        message.to_owned(),
    )
}
