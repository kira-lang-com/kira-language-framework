#![forbid(unsafe_code)]

mod hash;
mod interner;
mod type_id;

pub use hash::{ContentHash, ContentHasher};
pub use interner::{Interner, InternerError, StringId};
pub use type_id::TypeId;
