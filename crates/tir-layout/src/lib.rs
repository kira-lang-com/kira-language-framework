#![forbid(unsafe_code)]

mod aggregate;
mod engine;
mod model;

pub use engine::LayoutEngine;
pub use model::{
    DefinitionKind, EnumVariant, FieldLayout, Layout, LayoutError, LayoutField, LayoutRule,
    NominalDefinition, NominalLayout,
};

#[cfg(test)]
mod tests;
