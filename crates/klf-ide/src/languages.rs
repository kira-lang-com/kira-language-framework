//! The editor specs a server serves, chosen per file by extension. A
//! generated driver serves one language; `klf lsp` serves every kernel file
//! type from one process (`KLF-EDITOR.md` section 16).

use klf_facts::spec::EditorSpec;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Languages {
    default: Arc<EditorSpec>,
    by_extension: Vec<(String, Arc<EditorSpec>)>,
}

impl Languages {
    #[must_use]
    pub fn single(spec: EditorSpec) -> Self {
        Self {
            default: Arc::new(spec),
            by_extension: Vec::new(),
        }
    }

    /// Serves `spec` for files ending in `.extension`.
    #[must_use]
    pub fn with(mut self, extension: &str, spec: EditorSpec) -> Self {
        self.by_extension
            .push((extension.to_owned(), Arc::new(spec)));
        self
    }

    #[must_use]
    pub fn spec(&self, path: &Path) -> &Arc<EditorSpec> {
        let extension = path.extension().and_then(|extension| extension.to_str());
        self.by_extension
            .iter()
            .find(|(known, _)| Some(known.as_str()) == extension)
            .map_or(&self.default, |(_, spec)| spec)
    }

    fn all(&self) -> impl Iterator<Item = &Arc<EditorSpec>> {
        std::iter::once(&self.default).chain(self.by_extension.iter().map(|(_, spec)| spec))
    }

    /// The root markers of every language, in order and without repeats.
    #[must_use]
    pub fn root_markers(&self) -> Vec<String> {
        let mut markers: Vec<String> = Vec::new();
        for marker in self.all().flat_map(|spec| &spec.root_markers) {
            if !markers.contains(marker) {
                markers.push(marker.clone());
            }
        }
        markers
    }

    /// The access operators of every language, which trigger completion.
    #[must_use]
    pub fn access_operators(&self) -> Vec<String> {
        let mut operators: Vec<String> = Vec::new();
        for operator in self.all().flat_map(|spec| spec.access_operators()) {
            if !operator.is_empty() && !operators.contains(&operator) {
                operators.push(operator);
            }
        }
        operators
    }
}
