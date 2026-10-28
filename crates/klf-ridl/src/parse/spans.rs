//! Where each declaration of a `.ridl` file sits, for tools that edit the
//! text or point into it: `--ridl-source` emission and the editor's
//! `@Source` links (`KLF-EDITOR.md` section 15.3).

use klf_source::Span;

/// The spans of one top-level declaration, in the order of
/// [`crate::Module::items`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemSpans {
    /// Where the declaration begins: its first annotation, or its keyword.
    pub start: u32,
    /// The declared name; `None` for an `extend`.
    pub name: Option<Span>,
    /// The string argument of each `@Source`, quotes included.
    pub sources: Vec<Span>,
}
