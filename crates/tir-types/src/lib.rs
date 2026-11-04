#![forbid(unsafe_code)]

mod canonical;
mod types;
mod validation;

pub use types::{
    Abi, AddressSpace, ClosureType, Convention, CooperativeMatrixUse, EntryKind, FloatType,
    FunctionType, MatrixScope, ParameterType, TextureAccess, TextureDimension, Type,
};
pub use validation::TypeError;
