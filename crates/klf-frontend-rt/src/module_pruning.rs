//! What of a module a native binary keeps: the functions it exports, and
//! the definitions something it runs reaches.

use klf_kir::Module;

/// The functions a `program.export` attribute names that the module
/// defines: an executable exports each under its own name, as a C program
/// does for the modules it loads.
pub(crate) fn program_exports(module: &Module) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for attribute in &module.attributes {
        if attribute.name != "program.export" {
            continue;
        }
        let Some(name) = attribute.value.as_deref() else {
            continue;
        };
        if name == "main" || names.contains(&name) {
            continue;
        }
        if module
            .functions
            .iter()
            .any(|function| function.name.as_str() == name && !function.foreign && !function.blocks.is_empty())
        {
            names.push(name);
        }
    }
    names
}

/// The module without the function definitions nothing reaches from `main`,
/// the initializers or an exported function: a program's own copy of every
/// inline function its headers define is mostly unused, and code generation
/// is where the time goes. Foreign declarations stay.
pub(crate) fn prune_unreachable(mut module: Module) -> Module {
    use klf_kir::{FunctionAttribute, Linkage};
    let exported: std::collections::HashSet<String> = module
        .attributes
        .iter()
        .filter(|attribute| attribute.name == "program.export")
        .filter_map(|attribute| attribute.value.clone())
        .collect();
    let roots = module.functions.iter().filter(|function| {
        function.name.as_str() == "main"
            || exported.contains(function.name.as_str())
            || function.linkage == Linkage::Exported
            || function
                .attributes
                .contains(&FunctionAttribute::Initializer)
    });
    let reached = module.reachable_functions(roots.map(|function| &function.name));
    let mut position = 0;
    module.functions.retain(|function| {
        let kept = function.foreign || function.blocks.is_empty() || reached.contains(&position);
        position += 1;
        kept
    });
    module
}
