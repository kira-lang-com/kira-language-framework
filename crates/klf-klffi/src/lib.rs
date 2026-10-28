//! `klf-klffi` (layer 4): the `.klffi` C ABI boundary (section 26, grammar
//! A.3). Parses `.klffi` files, prints them, generates them from C headers, and
//! maps `@FFI.Extern` functions to the KIR foreign declarations that
//! `call.foreign` resolves against.

mod c_record;
mod cdecl;
mod cgen;
mod emit;
mod kir;
mod model;
mod names;
mod parse;
mod preprocess;

pub use c_record::{CPlacement, CRecord, LayoutError};
pub use cgen::{Remark, generate, generate_withholding};
pub use emit::emit;
pub use kir::{ffi_type_to_kir, foreign_declarations, record_layout, type_declarations};
pub use model::{
    Annotation, AnnotationKind, FfiConstant, FfiField, FfiFunction, FfiItem, FfiModule, FfiStruct,
    FfiType,
};
pub use parse::{FfiError, parse};
pub use preprocess::{
    Define, Environment, HeaderError, Platform, Predefined, Preprocessed, preprocess,
};
