//! RIDL: the native ABI and its `.ridl` interface format (sections 19-24).
//!
//! This crate owns the RIDL type system, the data-layout engine (section 21),
//! and the machinery that grows from it: parsing, emission, symbol mangling,
//! module metadata, and ABI freeze/apply.
//!
//! `too_many_lines` and `match_same_arms` are allowed: the IR writers match
//! over every passing and result shape, which reads best whole and with one
//! arm per shape. `result_large_err` is allowed because a parse error is a
//! full [`klf_diag::Diagnostic`] with its labels, built once on the way out.
#![allow(
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::result_large_err
)]

pub mod abi;
pub mod bindgen;
pub mod cc;
pub mod descriptor;
pub mod emit;
pub mod layout;
pub mod llvm;
pub mod demangle;
pub mod mangle;
pub mod meta;
pub mod module;
pub mod parse;
pub mod sha256;
pub mod types;

pub use emit::{canonical_text, emit};
pub use layout::{Layout, LayoutEngine, LayoutError};
pub use module::{Item, ItemKind, Module};
pub use parse::{ItemSpans, parse, parse_spanned};
pub use types::{Case, Decl, EnumDecl, Field, Registry, Scalar, StructDecl, Type};
