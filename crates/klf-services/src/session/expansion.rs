//! Macro expansions (`KLF-EDITOR.md` section 13.6): the virtual file holding
//! what one macro invocation expanded to, so an editor can show it.

use super::Session;
use klf_core::FileId;
use klf_source::Span;

/// What the macro `name`, invoked at `origin`, expanded to: the text of
/// `file`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expansion {
    pub file: FileId,
    pub origin: Span,
    pub name: String,
}

impl Session {
    /// Records an expansion once, however often a phase reports it.
    pub fn record_expansion(&mut self, expansion: Expansion) {
        if !self.expansions.contains(&expansion) {
            self.expansions.push(expansion);
        }
    }

    #[must_use]
    pub fn expansions(&self) -> &[Expansion] {
        &self.expansions
    }
}
