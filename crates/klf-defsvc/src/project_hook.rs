//! One call of a definition's `project` hook (`KLF-EDITOR.md` section 5.3):
//! the session's services, the path it asks about, and the project it hands
//! back field by field through `klf.editor.project_field`.

use crate::drive::prepare_phase;
use crate::driver_arguments::register_driver_arguments;
use crate::{DefinitionServiceBridge, ServiceSink, settle_phase_diagnostics};
use klf_diag::RuntimeDiagnostic;
use klf_rt::MAX_SERVICE_BUFFER_BYTES;
use klf_services::{DefinitionPhase, DefinitionPhaseArguments, Session};
use std::sync::{Arc, Mutex};

/// The KIR entry the KLF Kira lowerer generates around a `project` hook. It
/// takes the session and the argument count (1, the path) and returns 0 for
/// `None`, 1 for `Some`.
pub const PROJECT_ENTRY: &str = "__klf_project";

/// The project a definition placed a file in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HookProject {
    pub name: String,
    pub root: String,
    pub arguments: Vec<String>,
    pub watch: Vec<String>,
    /// Where build on save writes (section 14.6).
    pub output: Option<String>,
}

type Pushed = Arc<Mutex<Result<HookProject, String>>>;

/// The services and words of one `project` call.
pub struct ProjectCall {
    pub bridge: DefinitionServiceBridge,
    /// The session handle and the argument count.
    pub words: [u64; 2],
    /// The text of the bridge's file, which the runtime's diagnostics quote.
    pub source: String,
    path: String,
    pushed: Pushed,
}

impl ProjectCall {
    /// Opens a call asking where `path` belongs. The hook runs with the
    /// session as `units` sees it, before any unit exists.
    ///
    /// # Errors
    /// Returns a message when the session cannot be handed to the hook.
    pub fn open(session: &mut Session, path: &str) -> Result<Self, String> {
        let arguments = DefinitionPhaseArguments::Units { inputs: Vec::new() };
        let (bridge, words, source) =
            prepare_phase(session, DefinitionPhase::Units, &arguments, false)?;
        let handle = *words.first().ok_or("the session has no handle")?;
        Ok(Self {
            bridge,
            words: [handle, 1],
            source,
            path: path.to_owned(),
            pushed: Arc::new(Mutex::new(Ok(HookProject::default()))),
        })
    }

    /// Registers the session's services, the path, and the field collector.
    pub fn register(&self, services: &mut ServiceSink) {
        self.bridge.register(services);
        register_driver_arguments(vec![self.path.clone()], services);
        let sink = Arc::clone(&self.pushed);
        services.register_with_memory("klfsvc_editor_project_field", move |words, memory| {
            let [kind, address, length] = words else {
                return 0;
            };
            let Ok(mut pushed) = sink.lock() else {
                return 0;
            };
            let Ok(project) = pushed.as_mut() else {
                return 0;
            };
            let Some(length) = usize::try_from(*length)
                .ok()
                .filter(|length| *length <= MAX_SERVICE_BUFFER_BYTES)
            else {
                *pushed = Err("project returned a field over the buffer limit".into());
                return 0;
            };
            let mut bytes = vec![0; length];
            if !memory.read_buffer(*address, &mut bytes) {
                *pushed = Err("project returned an unreadable field".into());
                return 0;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                *pushed = Err("project returned a field that is not UTF-8".into());
                return 0;
            };
            match kind {
                0 => project.name = text,
                1 => project.root = text,
                2 => project.arguments.push(text),
                3 => project.watch.push(text),
                4 => project.output = Some(text),
                _ => return 0,
            }
            1
        });
    }

    /// Settles what the hook reported and decodes its result word.
    ///
    /// # Errors
    /// Returns a message when the hook reported an error, returned something
    /// other than 0 or 1, or handed back a field that could not be read.
    pub fn finish(
        self,
        session: &mut Session,
        reported: &[RuntimeDiagnostic],
        word: u64,
    ) -> Result<Option<HookProject>, String> {
        self.bridge.commit_records(session)?;
        settle_phase_diagnostics(session, self.bridge.file, reported)?;
        let pushed = self
            .pushed
            .lock()
            .map_err(|_| "project field lock poisoned".to_owned())?
            .clone()?;
        match word {
            0 => Ok(None),
            1 => Ok(Some(pushed)),
            other => Err(format!(
                "project entry returned {}, expected 0 or 1",
                other.cast_signed()
            )),
        }
    }
}
