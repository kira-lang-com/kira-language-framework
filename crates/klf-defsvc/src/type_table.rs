//! The type declarations a definition builds, each with its owning module,
//! indexed by module and name. Builders look a nominal type up on almost
//! every aggregate they lower, so the lookup must not scan the table.

use klf_core::Name;
use klf_kir::TypeDeclaration;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

#[derive(Default)]
pub(crate) struct TypeTable {
    declarations: Vec<(u64, TypeDeclaration)>,
    by_name: HashMap<u64, HashMap<Name, usize>>,
}

impl TypeTable {
    /// Adds `declaration` to `module`. A later declaration of a name the
    /// module already has does not shadow the first, as a scan would not.
    pub(crate) fn push(&mut self, module: u64, declaration: TypeDeclaration) {
        let index = self.declarations.len();
        self.by_name
            .entry(module)
            .or_default()
            .entry(declaration.name.clone())
            .or_insert(index);
        self.declarations.push((module, declaration));
    }

    /// The index of the first declaration of `name` in `module`.
    pub(crate) fn find(&self, module: u64, name: &str) -> Option<usize> {
        self.by_name.get(&module)?.get(name).copied()
    }

    /// The declaration of `name` in `module`.
    pub(crate) fn declaration(&self, module: u64, name: &str) -> Option<&TypeDeclaration> {
        self.find(module, name)
            .map(|index| &self.declarations[index].1)
    }

    pub(crate) fn into_vec(self) -> Vec<(u64, TypeDeclaration)> {
        self.declarations
    }
}

/// Reads and in-place edits go to the declarations directly. An edit keeps a
/// declaration's module and name, which the index relies on.
impl Deref for TypeTable {
    type Target = [(u64, TypeDeclaration)];

    fn deref(&self) -> &Self::Target {
        &self.declarations
    }
}

impl DerefMut for TypeTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.declarations
    }
}
