//! Reflection (`KLF-KSL.md` section 21): what a host binds against, as the
//! oracle's `KSLR1` document and `.resources` digest.
//!
//! The encoder and decoder dispatch over every record in one match each.
#![allow(
    clippy::cast_possible_truncation,
    clippy::match_same_arms,
    clippy::too_many_lines
)]

mod build;
mod digest;
mod kslr;
mod model;

pub use build::{Context, builtin_word, literal_text, reflect, type_name};
pub use digest::resource_digest;
pub use kslr::{MAGIC, decode, encode};
pub use model::{
    BindRecord, FieldRecord, Layout, LayoutField, OptionRecord, Reflection, Rename, ResourceRecord,
    StageRecord, TypeRecord,
};
