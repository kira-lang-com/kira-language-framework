use super::*;
use lsp_types::notification::LogMessage;
use lsp_types::{LogMessageParams, MessageType};

impl<'c> Server<'c> {
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

    pub(super) fn notify(&self, method: &str, params: &impl serde::Serialize) {
        let params = serde_json::to_value(params).unwrap_or_default();
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                method.to_owned(),
                params,
            )));
    }

    pub(super) fn send(&self, response: Response) {
        let _ = self.connection.sender.send(Message::Response(response));
    }
}

pub(super) fn owns(project: &Project, path: &Path) -> bool {
    if project.loose {
        project
            .arguments
            .first()
            .is_some_and(|file| Path::new(file) == path)
    } else {
        path.starts_with(&project.root)
    }
}
