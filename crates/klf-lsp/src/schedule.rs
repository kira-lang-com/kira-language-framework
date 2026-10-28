//! Which analysis runs next (`KLF-EDITOR.md` sections 4.3, 8.5 and 8.7).
//!
//! Analyses run on their own threads, at most as many at once as the
//! analyzer has lanes. A request the user waits on comes first, then any
//! other waiting request, then the diagnostics of projects with open
//! documents, which wait for a debounce after the last change. A change
//! cancels the running analysis of its project at once.

use klf_ide::{Analysis, AnalysisRequest, Analyzer, Cancellation, Project};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long diagnostics wait after a change before they are computed.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(50);

/// Requests the user waits on, served before anything else.
const INTERACTIVE: &[&str] = &[
    "textDocument/completion",
    "textDocument/hover",
    "textDocument/signatureHelp",
    "textDocument/definition",
    "textDocument/declaration",
    "textDocument/typeDefinition",
    "textDocument/implementation",
    "textDocument/documentHighlight",
    "textDocument/references",
    "textDocument/prepareRename",
    "textDocument/rename",
];

/// Requests the client asks again for after a change, answered with
/// `ContentModified` when their analysis is cancelled (section 8.5 item 3).
const ASKED_AGAIN: &[&str] = &[
    "textDocument/semanticTokens/full",
    "textDocument/semanticTokens/full/delta",
    "textDocument/semanticTokens/range",
    "textDocument/inlayHint",
    "textDocument/codeLens",
    "textDocument/documentSymbol",
    "textDocument/foldingRange",
];

pub(crate) fn interactive(method: &str) -> bool {
    INTERACTIVE.contains(&method)
}

pub(crate) fn asked_again(method: &str) -> bool {
    ASKED_AGAIN.contains(&method)
}

struct Running {
    generation: u64,
    cancellation: Arc<Cancellation>,
    cancelled_at: Option<Instant>,
}

/// An analysis that ended, current or not.
pub(crate) struct Finished {
    pub project: Project,
    pub generation: u64,
    pub analysis: Analysis,
    pub documents: Arc<HashMap<PathBuf, Arc<str>>>,
    /// From the cancellation to the analysis releasing its lane.
    pub released_after: Option<Duration>,
}

pub(crate) struct Scheduler {
    analyzer: Arc<dyn Analyzer>,
    lanes: usize,
    running: HashMap<Project, Running>,
    changed: HashMap<Project, Instant>,
    finished: (
        crossbeam_channel::Sender<Finished>,
        crossbeam_channel::Receiver<Finished>,
    ),
}

impl Scheduler {
    pub(crate) fn new(analyzer: Arc<dyn Analyzer>) -> Self {
        Self {
            lanes: analyzer.lanes().max(1),
            analyzer,
            running: HashMap::new(),
            changed: HashMap::new(),
            finished: crossbeam_channel::unbounded(),
        }
    }

    pub(crate) fn finished(&self) -> crossbeam_channel::Receiver<Finished> {
        self.finished.1.clone()
    }

    pub(crate) fn has_free_lane(&self) -> bool {
        self.running.len() < self.lanes
    }

    /// Whether an analysis of `project` runs now, cancelled or not.
    pub(crate) fn runs_any(&self, project: &Project) -> bool {
        self.running.contains_key(project)
    }

    /// Cancels the running analysis of `project`.
    pub(crate) fn cancel(&mut self, project: &Project) {
        if let Some(running) = self.running.get_mut(project)
            && running.cancelled_at.is_none()
        {
            running.cancelled_at = Some(Instant::now());
            running.cancellation.cancel();
        }
    }

    /// Records a change to `project`: its analysis is cancelled and its
    /// diagnostics wait for the debounce.
    pub(crate) fn changed(&mut self, project: &Project) {
        self.changed.insert(project.clone(), Instant::now());
        self.cancel(project);
    }

    /// Whether the debounce after the last change to `project` is over.
    pub(crate) fn settled(&self, project: &Project) -> bool {
        self.changed
            .get(project)
            .is_none_or(|at| at.elapsed() >= DEBOUNCE)
    }

    /// How long until the next debounce ends, if one is under way.
    pub(crate) fn next_wake(&mut self) -> Option<Duration> {
        self.changed.retain(|_, at| at.elapsed() < DEBOUNCE);
        self.changed
            .values()
            .map(|at| DEBOUNCE.saturating_sub(at.elapsed()))
            .min()
    }

    /// Starts analyzing `project` at `generation` on a thread of its own.
    pub(crate) fn start(
        &mut self,
        project: Project,
        generation: u64,
        documents: Arc<HashMap<PathBuf, Arc<str>>>,
        kir: bool,
    ) {
        let request = AnalysisRequest {
            directory: project.root.clone(),
            arguments: project.arguments.clone(),
            documents: documents
                .iter()
                .map(|(path, text)| (path.clone(), text.to_string()))
                .collect(),
            kir,
        };
        let cancellation = Arc::new(Cancellation::new());
        self.running.insert(
            project.clone(),
            Running {
                generation,
                cancellation: Arc::clone(&cancellation),
                cancelled_at: None,
            },
        );
        let analyzer = Arc::clone(&self.analyzer);
        let sender = self.finished.0.clone();
        std::thread::spawn(move || {
            let analysis = analyzer.analyze(&request, &cancellation);
            let _ = sender.send(Finished {
                project,
                generation,
                analysis,
                documents,
                released_after: None,
            });
        });
    }

    /// Takes `done` off the running analyses, noting how long it took to
    /// release its lane after it was cancelled.
    pub(crate) fn complete(&mut self, done: &mut Finished) {
        let ours = self
            .running
            .get(&done.project)
            .is_some_and(|running| running.generation == done.generation);
        if ours && let Some(running) = self.running.remove(&done.project) {
            done.released_after = running.cancelled_at.map(|at| at.elapsed());
        }
    }
}
