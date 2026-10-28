//! A read-only view of one project's analysis at one revision, with the texts
//! its offsets point into (`KLF-EDITOR.md` section 8.3).

use crate::analysis::Analysis;
use crate::languages::Languages;
use klf_core::FileId;
use klf_facts::lexical::Classifier;
use klf_facts::spec::EditorSpec;
use klf_facts::{FactPhase, Location, Token};
use klf_text::LineIndex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// A byte span of a file named by path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileSpan {
    pub path: PathBuf,
    pub start: u32,
    pub end: u32,
}

pub struct Snapshot {
    pub analysis: Arc<Analysis>,
    pub languages: Arc<Languages>,
    documents: Arc<HashMap<PathBuf, Arc<str>>>,
    disk: RefCell<HashMap<PathBuf, Option<Arc<str>>>>,
    indexes: RefCell<HashMap<PathBuf, Rc<LineIndex>>>,
}

impl Snapshot {
    /// `documents` are the open documents at the analysis's revision; every
    /// other file is read from disk when first needed.
    #[must_use]
    pub fn new(
        analysis: Arc<Analysis>,
        languages: Arc<Languages>,
        documents: Arc<HashMap<PathBuf, Arc<str>>>,
    ) -> Self {
        Self {
            analysis,
            languages,
            documents,
            disk: RefCell::new(HashMap::new()),
            indexes: RefCell::new(HashMap::new()),
        }
    }

    /// The editor spec of the language `path` is in.
    #[must_use]
    pub fn spec(&self, path: &Path) -> &EditorSpec {
        self.languages.spec(path)
    }

    #[must_use]
    pub fn text(&self, path: &Path) -> Option<Arc<str>> {
        if let Some(text) = self.documents.get(path) {
            return Some(Arc::clone(text));
        }
        self.disk
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| std::fs::read_to_string(path).ok().map(Arc::from))
            .clone()
    }

    #[must_use]
    pub fn line_index(&self, path: &Path) -> Option<Rc<LineIndex>> {
        if let Some(index) = self.indexes.borrow().get(path) {
            return Some(Rc::clone(index));
        }
        let index = Rc::new(LineIndex::new(&self.text(path)?));
        self.indexes
            .borrow_mut()
            .insert(path.to_path_buf(), Rc::clone(&index));
        Some(index)
    }

    #[must_use]
    pub fn file(&self, path: &Path) -> Option<FileId> {
        self.analysis.file(path)
    }

    #[must_use]
    pub fn path(&self, file: FileId) -> Option<&PathBuf> {
        self.analysis.path(file)
    }

    #[must_use]
    pub fn span(&self, location: Location) -> Option<FileSpan> {
        Some(FileSpan {
            path: self.path(location.file)?.clone(),
            start: location.start,
            end: location.end,
        })
    }

    /// The file's semantic tokens: what the editor spec classifies lexically,
    /// with every recorded token painted over it (section 11.2).
    #[must_use]
    pub fn tokens(&self, path: &Path) -> Vec<Token> {
        let base = self.lexical_tokens(path);
        match self.file(path) {
            Some(file) => self.analysis.facts.tokens_over(file, base),
            None => base,
        }
    }

    #[must_use]
    pub fn lexical_tokens(&self, path: &Path) -> Vec<Token> {
        let (Some(text), Ok(classifier)) = (self.text(path), Classifier::new(self.spec(path)))
        else {
            return Vec::new();
        };
        classifier
            .classify(&text)
            .into_iter()
            .map(|token| Token {
                start: token.start,
                end: token.end,
                class: token.class,
                modifiers: token.modifiers,
                phase: FactPhase::Units,
                sequence: 0,
            })
            .collect()
    }

    /// The identifier around `offset`: the bytes on both sides that can be
    /// part of a name.
    #[must_use]
    pub fn word_at(&self, path: &Path, offset: u32) -> Option<(u32, u32)> {
        let text = self.text(path)?;
        let bytes = text.as_bytes();
        let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
        let at = (offset as usize).min(bytes.len());
        let start = bytes[..at]
            .iter()
            .rposition(|byte| !word(*byte))
            .map_or(0, |index| index + 1);
        let end = bytes[at..]
            .iter()
            .position(|byte| !word(*byte))
            .map_or(bytes.len(), |index| at + index);
        (start < end).then(|| (offset_u32(start), offset_u32(end)))
    }
}

pub(crate) fn offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}
