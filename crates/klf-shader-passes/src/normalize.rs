//! Normalization (section 17.1 step 1): `auto` locations and neutral
//! bindings.

use klf_shader::{Binding, Module, ResourceBinding, Span, Type, TypeInner};

/// Assigns every `auto` location and every resource's neutral binding.
pub fn normalize(module: &mut Module) {
    assign_struct_locations(module);
    assign_argument_locations(module);
    assign_bindings(module);
}

fn assign_struct_locations(module: &mut Module) {
    let mut changed = false;
    let mut items: Vec<(Type, Span)> = module
        .types
        .iter()
        .map(|(handle, ty)| (ty.clone(), module.types.span(handle)))
        .collect();
    for (ty, _) in &mut items {
        let TypeInner::Struct { members, .. } = &mut ty.inner else {
            continue;
        };
        if !members.iter().any(|member| {
            matches!(
                member.binding,
                Some(Binding::Location { location: None, .. })
            )
        }) {
            continue;
        }
        let mut next = 0;
        for member in members.iter_mut() {
            if let Some(Binding::Location { location, .. }) = &mut member.binding {
                let assigned = *location.get_or_insert(next);
                next = assigned + 1;
            }
        }
        changed = true;
    }
    if changed {
        module.types.rebuild(items);
    }
}

fn assign_argument_locations(module: &mut Module) {
    for entry in &mut module.entry_points {
        let mut next = 0;
        for argument in &mut entry.function.arguments {
            if let Some(Binding::Location { location, .. }) = &mut argument.binding {
                next = *location.get_or_insert(next) + 1;
            }
        }
        if let Some(result) = &mut entry.function.result
            && let Some(Binding::Location { location, .. }) = &mut result.binding
        {
            location.get_or_insert(0);
        }
    }
}

fn assign_bindings(module: &mut Module) {
    for shader in &module.shaders {
        for (group_index, group) in shader.groups.iter().enumerate() {
            for (position, global) in group.globals.iter().enumerate() {
                let variable = &mut module.globals[*global];
                let binding = variable.universal_slot().unwrap_or(position as u32);
                variable.binding = Some(ResourceBinding {
                    group: group_index as u32,
                    binding,
                });
            }
        }
    }
}
