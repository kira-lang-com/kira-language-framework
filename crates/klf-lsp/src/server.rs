//! The server loop (`KLF-EDITOR.md` sections 8 and 10): documents, projects,
//! the analyses the scheduler runs, and requests that wait for the analysis
//! of their project's current revision.

use crate::convert;
use crate::handlers;
use crate::schedule::{Finished, Scheduler, asked_again, interactive};
use crate::views::Views;
use klf_ide::{Analysis, Analyzer, Formatter, Interruption, Languages, Project, Snapshot};
use klf_text::{Document, Encoding};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    LogMessage, Notification as _, PublishDiagnostics, ShowMessage,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, LogMessageParams, MessageType, PublishDiagnosticsParams,
    ShowMessageParams,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a server serves.
pub struct ServerConfig {
    /// The driver's name, used as the diagnostic source.
    pub name: String,
    pub version: String,
    /// The editor spec of each file type served.
    pub languages: Languages,
    pub analyzer: Arc<dyn Analyzer>,
    /// Serves formatting when present (`KLF-FORMAT.md` section 6).
    pub formatter: Option<Arc<dyn Formatter>>,
}

/// What the client and server agreed on at `initialize`.
pub(crate) struct Agreement {
    pub encoding: Encoding,
    pub multiline_tokens: bool,
    pub folders: Vec<PathBuf>,
}

#[derive(Default)]
struct ProjectState {
    /// Increases whenever a file of the project changes.
    generation: u64,
    analyzed: Option<Analyzed>,
    published: HashSet<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct Analyzed {
    generation: u64,
    pub analysis: Arc<Analysis>,
    pub documents: Arc<HashMap<PathBuf, Arc<str>>>,
}

pub(crate) struct Server<'c> {
    connection: &'c Connection,
    config: ServerConfig,
    pub languages: Arc<Languages>,
    pub agreement: Agreement,
    documents: HashMap<PathBuf, (i32, Document)>,
    projects: HashMap<Project, ProjectState>,
    scheduler: Scheduler,
    waiting: Vec<(Project, Request)>,
    /// The `KLF-LSP-9001` keys already shown (section 25).
    shown: HashSet<String>,
    /// The virtual documents of section 17, one set per project.
    pub(crate) views: Views,
    /// The `klf/status` last sent, so only a change is sent again.
    reported: Option<serde_json::Value>,
    /// The projects someone asked for KIR of (section 13.7); only their
    /// analyses lower to KIR text.
    kir: HashSet<Project>,
}

impl<'c> Server<'c> {
    pub(crate) fn new(
        connection: &'c Connection,
        config: ServerConfig,
        agreement: Agreement,
    ) -> Self {
        Self {
            connection,
            languages: Arc::new(config.languages.clone()),
            agreement,
            documents: HashMap::new(),
            projects: HashMap::new(),
            scheduler: Scheduler::new(Arc::clone(&config.analyzer)),
            waiting: Vec::new(),
            shown: HashSet::new(),
            views: Views::default(),
            reported: None,
            kir: HashSet::new(),
            config,
        }
    }

    pub(crate) fn run(&mut self) -> Result<(), String> {
        let finished = self.scheduler.finished();
        loop {
            let wake = self
                .scheduler
                .next_wake()
                .map_or_else(crossbeam_channel::never, crossbeam_channel::after);
            crossbeam_channel::select! {
                recv(self.connection.receiver) -> message => {
                    let Ok(message) = message else { return Ok(()) };
                    if self.message(message)? {
                        return Ok(());
                    }
                }
                recv(finished) -> done => {
                    if let Ok(done) = done {
                        self.finish(done);
                    }
                }
                recv(wake) -> _ => {}
            }
            if self.connection.receiver.is_empty() {
                self.schedule();
                self.report_status();
            }
        }
    }

    /// Handles one message; true once the client asked the server to exit.
    fn message(&mut self, message: Message) -> Result<bool, String> {
        match message {
            Message::Request(request) => {
                if self
                    .connection
                    .handle_shutdown(&request)
                    .map_err(|error| error.to_string())?
                {
                    let projects: Vec<Project> = self.projects.keys().cloned().collect();
                    for project in &projects {
                        self.scheduler.cancel(project);
                    }
                    return Ok(true);
                }
                self.request(request);
            }
            Message::Notification(notification) => self.notification(notification),
            Message::Response(_) => {}
        }
        Ok(false)
    }

    fn request(&mut self, request: Request) {
        if let Some(response) = crate::views::answer(self, &request) {
            self.send(response);
            return;
        }
        if crate::formatting::METHODS.contains(&request.method.as_str()) {
            let response = crate::formatting::answer(self, &request);
            self.send(response);
            return;
        }
        if request.method == "workspace/symbol" {
            let response = handlers::workspace_symbols(self, &request);
            self.send(response);
            return;
        }
        let Some(path) = handlers::request_path(&request) else {
            let response = handlers::answer(self, None, &request);
            self.send(response);
            return;
        };
        let project = self.project(&path);
        if request.method == "klf/kir" && self.kir.insert(project.clone()) {
            // The analysis at hand has no KIR; the next one will.
            self.changed(&path);
        }
        if self.fresh(&project).is_some() {
            self.answer(&project, &request);
        } else {
            self.waiting.push((project, request));
        }
    }

    fn answer(&self, project: &Project, request: &Request) {
        let snapshot = self.fresh(project).map(|analyzed| self.snapshot(analyzed));
        let response = handlers::answer(self, snapshot.as_ref(), request);
        self.send(response);
    }

    pub(crate) fn snapshot(&self, analyzed: &Analyzed) -> Snapshot {
        Snapshot::new(
            Arc::clone(&analyzed.analysis),
            Arc::clone(&self.languages),
            Arc::clone(&analyzed.documents),
        )
    }

    /// Every project's latest analysis, current or not.
    pub(crate) fn analyses(&self) -> impl Iterator<Item = &Analyzed> {
        self.projects
            .values()
            .filter_map(|state| state.analyzed.as_ref())
    }

    fn fresh(&self, project: &Project) -> Option<&Analyzed> {
        let state = self.projects.get(project)?;
        state
            .analyzed
            .as_ref()
            .filter(|analyzed| analyzed.generation == state.generation)
    }

    fn notification(&mut self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
                    && let Some(path) = convert::path(&params.text_document.uri)
                {
                    self.documents.insert(
                        path.clone(),
                        (
                            params.text_document.version,
                            Document::new(params.text_document.text),
                        ),
                    );
                    self.project(&path);
                    self.changed(&path);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
                    && let Some(path) = convert::path(&params.text_document.uri)
                    && let Some((version, document)) = self.documents.get_mut(&path)
                {
                    *version = params.text_document.version;
                    for change in params.content_changes {
                        let range = change.range.map(convert::text_range);
                        if document
                            .edit(range, &change.text, self.agreement.encoding)
                            .is_err()
                        {
                            *document = Document::new(document.text().to_owned());
                        }
                    }
                    self.changed(&path);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
                    && let Some(path) = convert::path(&params.text_document.uri)
                {
                    self.documents.remove(&path);
                    self.changed(&path);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWatchedFilesParams>(notification.params)
                {
                    for change in params.changes {
                        if let Some(path) = convert::path(&change.uri) {
                            self.changed(&path);
                        }
                    }
                }
            }
            "$/cancelRequest" => {
                let id = notification.params.get("id").cloned();
                if let Some(index) = self
                    .waiting
                    .iter()
                    .position(|(_, request)| serde_json::to_value(&request.id).ok() == id)
                {
                    let (project, request) = self.waiting.remove(index);
                    self.send(Response::new_err(
                        request.id,
                        ErrorCode::RequestCanceled as i32,
                        "cancelled".to_owned(),
                    ));
                    let shared = self.waiting.iter().any(|(other, _)| *other == project)
                        || self.documents.keys().any(|path| owns(&project, path));
                    if !shared {
                        self.scheduler.cancel(&project);
                    }
                }
            }
            _ => {}
        }
    }

    /// `klf/status` (section 17.2), sent when it differs from the last.
    fn report_status(&mut self) {
        let status = crate::views::status_of(self);
        if self.reported.as_ref() != Some(&status) {
            self.notify("klf/status", &status);
            self.reported = Some(status);
        }
    }

    /// `klf/reloadProjects`: forgets every project, so each open document's
    /// project is resolved again from the files on disk and analyzed anew.
    /// Diagnostics already published are withdrawn, and the requests
    /// waiting on a forgotten project are answered `ContentModified`.
    pub(crate) fn reload_projects(&mut self) {
        let forgotten = std::mem::take(&mut self.projects);
        for (project, state) in forgotten {
            self.scheduler.cancel(&project);
            for path in state.published {
                if let Some(uri) = convert::uri(&path) {
                    let params = PublishDiagnosticsParams {
                        uri,
                        diagnostics: Vec::new(),
                        version: None,
                    };
                    self.notify(PublishDiagnostics::METHOD, &params);
                }
            }
        }
        for (_, request) in std::mem::take(&mut self.waiting) {
            self.send(Response::new_err(
                request.id,
                ErrorCode::ContentModified as i32,
                "the projects were reloaded".to_owned(),
            ));
        }
        let open: Vec<PathBuf> = self.documents.keys().cloned().collect();
        for path in &open {
            self.project(path);
            self.changed(path);
        }
    }

    /// The project of `path`, registered if it is new.
    fn project(&mut self, path: &Path) -> Project {
        let project = klf_ide::project::resolve(
            path,
            &self.agreement.folders,
            &self.languages.root_markers(),
        );
        self.projects.entry(project.clone()).or_default();
        project
    }

    /// Marks every project `path` belongs to as changed and cancels its
    /// running analysis. The requests waiting on it that the client asks
    /// again for are answered `ContentModified`; the others wait for the
    /// analysis of the new revision.
    fn changed(&mut self, path: &Path) {
        let changed: Vec<Project> = self
            .projects
            .keys()
            .filter(|project| owns(project, path))
            .cloned()
            .collect();
        for project in &changed {
            if let Some(state) = self.projects.get_mut(project) {
                state.generation += 1;
            }
            self.scheduler.changed(project);
        }
        let (dropped, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(project, request)| {
                changed.contains(project) && asked_again(&request.method)
            });
        self.waiting = kept;
        for (_, request) in dropped {
            self.send(Response::new_err(
                request.id,
                ErrorCode::ContentModified as i32,
                "the document changed".to_owned(),
            ));
        }
    }

    /// Starts analyses while lanes are free: first for projects a request
    /// the user waits on needs, then for any waiting request, then for
    /// projects with open documents once their debounce is over.
    fn schedule(&mut self) {
        while self.scheduler.has_free_lane() {
            let Some(project) = self.next_project() else {
                return;
            };
            let generation = self
                .projects
                .get(&project)
                .map_or(0, |state| state.generation);
            let documents: Arc<HashMap<PathBuf, Arc<str>>> = Arc::new(
                self.documents
                    .iter()
                    .map(|(path, (_, document))| (path.clone(), Arc::from(document.text())))
                    .collect(),
            );
            let kir = self.kir.contains(&project);
            self.scheduler.start(project, generation, documents, kir);
        }
    }

    fn next_project(&self) -> Option<Project> {
        let ready =
            |project: &Project| self.fresh(project).is_none() && !self.scheduler.runs_any(project);
        let waiting = |wanted: fn(&str) -> bool| {
            self.waiting
                .iter()
                .filter(|(_, request)| wanted(&request.method))
                .map(|(project, _)| project)
                .find(|project| ready(project))
        };
        waiting(interactive)
            .or_else(|| waiting(|_| true))
            .or_else(|| {
                self.documents
                    .keys()
                    .filter_map(|path| self.projects.keys().find(|project| owns(project, path)))
                    .find(|project| ready(project) && self.scheduler.settled(project))
            })
            .cloned()
    }

    fn finish(&mut self, mut done: Finished) {
        self.scheduler.complete(&mut done);
        if let Some(Interruption::Cancelled) = &done.analysis.interrupted {
            let released = done
                .released_after
                .map_or_else(String::new, |after| format!(" in {} ms", after.as_millis()));
            let note = done
                .analysis
                .failure
                .as_ref()
                .map_or_else(String::new, |failure| format!("; {failure}"));
            self.log(&format!(
                "{}: analysis of {} cancelled{released}{note}",
                self.config.name,
                done.project.root.display()
            ));
            return;
        }
        let current = self
            .projects
            .get(&done.project)
            .map(|state| state.generation);
        if current != Some(done.generation) {
            return;
        }
        if let Some(failure) = &done.analysis.failure {
            self.log(&format!("{}: {failure}", self.config.name));
        }
        let trap = match &done.analysis.interrupted {
            Some(Interruption::Trapped { code, query }) => Some(self.trapped(code, query)),
            _ => None,
        };
        let Some(state) = self.projects.get_mut(&done.project) else {
            return;
        };
        state.analyzed = Some(Analyzed {
            generation: done.generation,
            analysis: Arc::new(done.analysis),
            documents: done.documents,
        });
        self.publish(&done.project);
        let moved = self
            .latest(&done.project)
            .map(|analyzed| self.views.refresh(&done.project, &analyzed.analysis))
            .unwrap_or_default();
        if !moved.is_empty() {
            self.notify(
                "klf/shaderOutputChanged",
                &serde_json::json!({ "uris": moved }),
            );
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(project, _)| *project == done.project);
        self.waiting = waiting;
        for (project, request) in ready {
            match &trap {
                Some(message) => self.send(Response::new_err(
                    request.id,
                    ErrorCode::RequestFailed as i32,
                    message.clone(),
                )),
                None => self.answer(&project, &request),
            }
        }
    }

    /// Reports a trap in definition code as `KLF-LSP-9001` (sections 8.6 and
    /// 25): logged each time, shown once per query key.
    fn trapped(&mut self, code: &str, query: &str) -> String {
        let message = format!(
            "KLF-LSP-9001: definition `{}` trapped with {code} in {query}",
            self.config.name
        );
        self.notify(
            LogMessage::METHOD,
            &LogMessageParams {
                typ: MessageType::ERROR,
                message: message.clone(),
            },
        );
        if self.shown.insert(format!("{query}\t{code}")) {
            self.notify(
                ShowMessage::METHOD,
                &ShowMessageParams {
                    typ: MessageType::WARNING,
                    message: message.clone(),
                },
            );
        }
        message
    }

    fn publish(&mut self, project: &Project) {
        let Some(analyzed) = self.fresh(project).cloned() else {
            return;
        };
        let snapshot = self.snapshot(&analyzed);
        let mut paths: HashSet<PathBuf> = self
            .documents
            .keys()
            .filter(|path| owns(project, path))
            .cloned()
            .collect();
        paths.extend(
            analyzed
                .analysis
                .diagnostics
                .iter()
                .filter_map(|diagnostic| analyzed.analysis.path(diagnostic.file).cloned()),
        );
        let previous = self
            .projects
            .get_mut(project)
            .map(|state| std::mem::replace(&mut state.published, paths.clone()))
            .unwrap_or_default();
        paths.extend(previous);
        for path in paths {
            let Some(uri) = convert::uri(&path) else {
                continue;
            };
            let diagnostics = handlers::diagnostics(self, &snapshot, &path);
            let version = self.documents.get(&path).map(|(version, _)| *version);
            let params = PublishDiagnosticsParams {
                uri,
                diagnostics,
                version,
            };
            self.notify(PublishDiagnostics::METHOD, &params);
        }
    }

    pub(crate) fn log(&self, message: &str) {
        let params = LogMessageParams {
            typ: MessageType::LOG,
            message: message.to_owned(),
        };
        self.notify(LogMessage::METHOD, &params);
    }

    pub(crate) fn source(&self) -> &str {
        &self.config.name
    }

    /// The project a URI's file belongs to, when one is registered.
    pub(crate) fn project_naming(&self, uri: &str) -> Option<Project> {
        use std::str::FromStr as _;
        let path = convert::path(&lsp_types::Uri::from_str(uri).ok()?)?;
        self.projects
            .keys()
            .find(|project| owns(project, &path))
            .cloned()
    }

    /// A project's latest analysis, current or not.
    pub(crate) fn latest(&self, project: &Project) -> Option<&Analyzed> {
        self.projects.get(project)?.analyzed.as_ref()
    }

    pub(crate) fn project_count(&self) -> usize {
        self.projects.len()
    }

    pub(crate) fn any_failure(&self) -> bool {
        self.analyses()
            .any(|analyzed| analyzed.analysis.failure.is_some())
    }

    /// Whether any project is still waiting out its debounce (section 8.7).
    pub(crate) fn any_indexing(&self) -> bool {
        self.projects
            .keys()
            .any(|project| !self.scheduler.settled(project))
    }

    /// The text of an open document.
    pub(crate) fn document(&self, path: &Path) -> Option<&str> {
        self.documents
            .get(path)
            .map(|(_, document)| document.text())
    }

    pub(crate) fn formatter(&self) -> Option<&Arc<dyn Formatter>> {
        self.config.formatter.as_ref()
    }

    fn notify(&self, method: &str, params: &impl serde::Serialize) {
        let params = serde_json::to_value(params).unwrap_or_default();
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                method.to_owned(),
                params,
            )));
    }

    fn send(&self, response: Response) {
        let _ = self.connection.sender.send(Message::Response(response));
    }
}

fn owns(project: &Project, path: &Path) -> bool {
    if project.loose {
        project
            .arguments
            .first()
            .is_some_and(|file| Path::new(file) == path)
    } else {
        path.starts_with(&project.root)
    }
}

/// A response for a request the server does not handle.
pub(crate) fn unhandled(id: RequestId, method: &str) -> Response {
    Response::new_err(
        id,
        ErrorCode::MethodNotFound as i32,
        format!("unhandled method {method}"),
    )
}
