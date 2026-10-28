//! The `ridlcc` entries of a library with a described RIDL surface
//! (sections 22 to 24), as LLVM IR for the bundled clang. The IR comes from
//! `klf_ridl::llvm`; this names the frontend functions behind each export.

use crate::ridl_surface::Surface;
use std::collections::BTreeMap;

/// The LLVM IR of `surface`'s entries, descriptors and registration, built
/// for `target`. `defined` names the object symbol of a KIR function.
pub(crate) fn source(
    surface: &Surface,
    target: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let callees: BTreeMap<String, String> = surface
        .entries
        .iter()
        .map(|(path, function)| (path.clone(), defined(function)))
        .collect();
    klf_ridl::llvm::entries(
        &surface.module,
        &callees,
        &surface.destroys,
        target,
        defined,
    )
}
