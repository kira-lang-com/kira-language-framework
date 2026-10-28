//! Editor facts (`KLF-EDITOR.md` section 6): what a definition's phases
//! record while they run, stored per file, and the questions editor features
//! ask of them. Also the definition's editor spec (section 5.2) and the
//! pattern dialect it is written in (Appendix C).

pub mod codec;
mod facts_codec;
mod kinds;
pub mod lexical;
pub mod pattern;
pub mod query;
pub mod spec;
mod spec_codec;
mod store;

pub use kinds::{
    AnalysisMode, FactPhase, FoldKind, HintKind, ReferenceRole, RunnableKind, SymbolKind,
    TokenClass, TokenModifier,
};
pub use query::{Location, SymbolTree};
pub use spec_codec::{enter_action_name, enter_action_of};
pub use store::{
    CallEdge, Entity, Facts, FileFacts, Hint, Link, LinkTarget, Reference, Runnable, SpanText,
    Symbol, Token,
};

#[cfg(test)]
mod tests;
