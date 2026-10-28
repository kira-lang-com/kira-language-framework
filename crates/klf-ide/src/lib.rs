//! The analysis host of the language server (`KLF-EDITOR.md` sections 8, 9
//! and 11): what an analysis is, how one runs, which project a file belongs
//! to, and the editor features answered from an analysis's facts.

pub mod analysis;
mod assist;
mod cancel;
mod languages;
mod navigate;
pub mod project;
mod snapshot;
pub mod worker;

pub use analysis::{
    Analysis, AnalysisRequest, Expansion, FileInfo, IdeDiagnostic, Interruption, KirFunction,
    ShaderOutput,
};
pub use assist::{CompletionItem, CompletionKind, HoverText, LinkDestination, SignatureHelp};
pub use cancel::Cancellation;
pub use languages::Languages;
pub use navigate::{Access, RenameEdit, SymbolItem};
pub use project::Project;
pub use snapshot::{FileSpan, Snapshot};
pub use worker::{Analyzer, FormatRequest, Formatter, ProcessAnalyzer, ProcessFormatter};
