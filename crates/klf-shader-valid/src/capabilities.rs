//! What a module uses that needs a capability (SV24): its types and its
//! stage interfaces.

use klf_shader::{Binding, BuiltIn, Capability, Module, Sampling, Span};
use std::collections::BTreeSet;

/// Each interface leaf that needs a capability: the capability, what
/// needs it, and the entry's span.
pub(crate) fn interface_uses(module: &Module) -> Vec<(Capability, &'static str, Span)> {
    let mut uses = Vec::new();
    for entry in &module.entry_points {
        let Ok(leaves) = crate::interface::leaves(module, entry) else {
            continue;
        };
        for leaf in leaves {
            let needed = match leaf.binding {
                Binding::BuiltIn(BuiltIn::SampleIndex) => {
                    Some((Capability::SampleShading, "sample_index"))
                }
                Binding::BuiltIn(BuiltIn::ClipDistances) => {
                    Some((Capability::ClipDistances, "clip_distances"))
                }
                Binding::Location {
                    sampling: Some(Sampling::Sample),
                    ..
                } => Some((Capability::SampleShading, "per-sample interpolation")),
                Binding::Location {
                    blend_src: Some(_), ..
                } => Some((Capability::DualSourceBlending, "blend_src")),
                _ => None,
            };
            if let Some((capability, what)) = needed {
                uses.push((capability, what, entry.span));
            }
        }
    }
    uses
}

/// The capabilities `module` uses, whether or not it declares them.
#[must_use]
pub fn used_capabilities(module: &Module) -> BTreeSet<Capability> {
    let mut used: BTreeSet<Capability> = module
        .types
        .iter()
        .flat_map(|(_, ty)| crate::types::type_capabilities(&ty.inner))
        .collect();
    used.extend(
        interface_uses(module)
            .into_iter()
            .map(|(capability, _, _)| capability),
    );
    used
}
