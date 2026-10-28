//! Editor analysis of the kernel's own file types, for `klf lsp`
//! (`KLF-EDITOR.md` section 16): the same host as a generated driver's server,
//! with native analyzers in place of a compiled definition.
//!
//! A project holding a `language.klfd` is a definition package and is
//! compiled whole; every other open `.klfd`, `.kir` or `.ridl` file is
//! analyzed on its own.

mod kir;
mod klfd;
mod ridl;
pub mod specs;

use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_facts::Facts;
use klf_ide::{Analysis, AnalysisRequest, Analyzer, Cancellation, IdeDiagnostic, Interruption};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use specs::languages;

/// The file a definition package's root holds.
pub const PACKAGE_MARKER: &str = "language.klfd";

/// Analyzes kernel files in the server's own process. The analyses run no
/// definition code, so there is nothing to trap.
pub struct KernelAnalyzer;

impl Analyzer for KernelAnalyzer {
    fn analyze(&self, request: &AnalysisRequest, cancellation: &Cancellation) -> Analysis {
        let documents: HashMap<PathBuf, &str> = request
            .documents
            .iter()
            .map(|(path, text)| (path.clone(), text.as_str()))
            .collect();
        let mut builder = Builder::new(&documents);
        let Some(argument) = request.arguments.first().map(PathBuf::from) else {
            return Analysis::failed("no file or directory to analyze");
        };
        let mut packaged = false;
        if argument.is_dir() && argument.join(PACKAGE_MARKER).exists() {
            klfd::package(&mut builder, &argument);
            packaged = true;
        }
        let mut loose: Vec<&PathBuf> = documents
            .keys()
            .filter(|path| **path == argument || path.starts_with(&argument))
            .collect();
        loose.sort();
        for path in loose {
            if cancellation.is_cancelled() {
                return Analysis::interrupted(Interruption::Cancelled);
            }
            match path.extension().and_then(|extension| extension.to_str()) {
                Some("klfd") if packaged => {}
                Some("klfd") => klfd::loose(&mut builder, path),
                Some("kir") => kir::file(&mut builder, path),
                Some("ridl" | "ridlsig") => ridl::file(&mut builder, path),
                _ => {}
            }
        }
        builder.finish()
    }

    /// Kernel analyses share nothing, so they run side by side, keeping one
    /// core for the protocol (section 8.7).
    fn lanes(&self) -> usize {
        std::thread::available_parallelism().map_or(1, |cores| cores.get().saturating_sub(1).max(1))
    }
}

/// An analysis being assembled from several sessions, each numbering its
/// files from zero.
pub(crate) struct Builder<'d> {
    documents: &'d HashMap<PathBuf, &'d str>,
    files: Vec<(FileId, PathBuf)>,
    diagnostics: Vec<IdeDiagnostic>,
    pub facts: Facts,
}

impl<'d> Builder<'d> {
    fn new(documents: &'d HashMap<PathBuf, &'d str>) -> Self {
        Self {
            documents,
            files: Vec::new(),
            diagnostics: Vec::new(),
            facts: Facts::new(),
        }
    }

    /// The text of `path`: the open document, else the file on disk.
    pub fn text(&self, path: &Path) -> Option<String> {
        match self.documents.get(path) {
            Some(text) => Some((*text).to_owned()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    /// The analysis's id for `path`.
    pub fn file(&mut self, path: &Path) -> FileId {
        if let Some((file, _)) = self.files.iter().find(|(_, known)| known == path) {
            return *file;
        }
        let file = FileId::from_u32(u32::try_from(self.files.len()).unwrap_or(u32::MAX));
        self.files.push((file, path.to_path_buf()));
        file
    }

    /// Adds `diagnostic`, whose spans name files through `files`.
    pub fn report(&mut self, diagnostic: &Diagnostic, files: &dyn Fn(FileId) -> Option<FileId>) {
        let Some(span) = diagnostic.primary_span else {
            return;
        };
        let Some(file) = files(span.file) else {
            return;
        };
        self.diagnostics.push(IdeDiagnostic {
            file,
            start: span.start,
            end: span.end,
            severity: diagnostic.severity,
            code: diagnostic.code.to_owned(),
            message: diagnostic.message.clone(),
            source: None,
        });
    }

    pub fn error(&mut self, file: FileId, start: u32, end: u32, code: &str, message: String) {
        self.diagnostics.push(IdeDiagnostic {
            file,
            start,
            end,
            severity: klf_diag::Severity::Error,
            code: code.to_owned(),
            message,
            source: None,
        });
    }

    fn finish(self) -> Analysis {
        Analysis {
            files: self.files,
            diagnostics: self.diagnostics,
            facts: self.facts,
            shader_outputs: Vec::new(),
            file_info: Vec::new(),
            kir: Vec::new(),
            expansions: Vec::new(),
            failure: None,
            interrupted: None,
        }
    }
}
