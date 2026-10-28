//! Stage selection (section 17.1 step 3).

use klf_shader::Module;

/// A copy of `module` holding only entry point `entry`, and the shader
/// declaration that names it.
#[must_use]
pub fn select_entry(module: &Module, entry: usize) -> Module {
    let mut selected = module.clone();
    selected.entry_points = vec![module.entry_points[entry].clone()];
    selected.shaders = module
        .shaders
        .iter()
        .filter(|shader| shader.entries.contains(&entry))
        .map(|shader| {
            let mut shader = shader.clone();
            shader.entries = vec![0];
            shader
        })
        .collect();
    selected
}
