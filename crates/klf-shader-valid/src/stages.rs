//! Whole-module rules: SV21, uniformity reporting (section 16.5), the
//! interface half of SV24, and resources reached outside their shader's
//! groups (SV13).

use crate::info::ModuleInfo;
use crate::{Options, ValidationError};
use klf_shader::{AddressSpace, Module, Stage};
use std::collections::BTreeSet;

fn interface_capabilities(module: &Module, problems: &mut Vec<ValidationError>) {
    for (capability, what, span) in crate::capabilities::interface_uses(module) {
        if !module.capabilities.contains(&capability) {
            problems.push(ValidationError::new(
                "SV24",
                format!("{what} needs capability `{}`", capability.name()),
                span,
            ));
        }
    }
}

/// The stages of the entries that reach each function.
fn reaching_stages(module: &Module, info: &ModuleInfo) -> Vec<BTreeSet<Stage>> {
    let mut stages = vec![BTreeSet::new(); module.functions.len()];
    for (index, entry) in module.entry_points.iter().enumerate() {
        for function in info.reachable(index) {
            stages[function.index()].insert(entry.stage);
        }
    }
    stages
}

pub(crate) fn check_stages(
    module: &Module,
    info: &mut ModuleInfo,
    options: Options,
    problems: &mut Vec<ValidationError>,
) {
    let reaching = reaching_stages(module, info);
    let report = |stages: &BTreeSet<Stage>,
                  function: &crate::FunctionInfo,
                  problems: &mut Vec<ValidationError>| {
        for (operation, span) in &function.stage_operations {
            if let Some(stage) = stages.iter().find(|stage| !operation.allowed(**stage)) {
                problems.push(ValidationError::new(
                    "SV21",
                    format!(
                        "{} is reached from a {} entry",
                        operation.name(),
                        stage.name()
                    ),
                    *span,
                ));
            }
        }
        for (what, span) in &function.non_uniform_operations {
            let mut problem = ValidationError::keyed(
                "uniformity",
                "shader.uniformity",
                format!("{what} in non-uniform control flow"),
                *span,
            );
            problem.warning = !options.wgsl;
            problems.push(problem);
        }
    };
    for (index, function) in info.functions.iter().enumerate() {
        report(&reaching[index], function, problems);
    }
    for (entry, function) in module.entry_points.iter().zip(&info.entries) {
        report(&BTreeSet::from([entry.stage]), function, problems);
    }
    interface_capabilities(module, problems);
    for (index, entry) in module.entry_points.iter().enumerate() {
        let uses = info.entry_globals(module, index);
        let immediates = module
            .globals
            .iter()
            .filter(|(handle, global)| {
                global.space == AddressSpace::Immediate && uses[handle.index()].any()
            })
            .count();
        if immediates > 1 {
            problems.push(ValidationError::new(
                "SV14",
                format!("entry `{}` uses two immediate blocks", entry.name),
                entry.span,
            ));
        }
        let Some(shader) = module.shader_of_entry(index) else {
            continue;
        };
        let grouped: BTreeSet<_> = shader
            .groups
            .iter()
            .flat_map(|group| group.globals.iter().copied())
            .collect();
        for (handle, global) in module.globals.iter() {
            if global.space.is_resource()
                && uses[handle.index()].any()
                && !grouped.contains(&handle)
            {
                problems.push(ValidationError::new(
                    "SV13",
                    format!(
                        "entry `{}` uses `{}`, which is in no group of `{}`",
                        entry.name, global.name, shader.name
                    ),
                    entry.span,
                ));
            }
        }
    }
}
