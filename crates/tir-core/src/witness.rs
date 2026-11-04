use crate::Type;
use klf_core::Name;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WitnessDeclaration {
    pub ty: Type,
    pub trait_name: Name,
    pub witnesses: Vec<Name>,
    pub associated: Vec<Type>,
}
