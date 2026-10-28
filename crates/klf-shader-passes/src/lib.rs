//! The KSIR passes (`KLF-KSL.md` section 17): normalization, stage
//! selection, override baking, integer guards, workgroup zeroing and
//! compaction, and the pipeline that runs them for one target.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

pub mod bounds;
pub mod compact;
pub mod guards;
pub mod halves;
pub mod normalize;
pub mod overrides;
pub mod rewrite;
pub mod select;
pub mod zeroing;

pub use bounds::Bounds;
pub use normalize::normalize;
pub use overrides::Baked;

use klf_shader::{Capability, Module, Span, Target};
use klf_shader_valid::ValidationError;
use std::collections::BTreeMap;

/// How integer edge cases behave (section 15.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IntegerDivision {
    #[default]
    Defined,
    Native,
}

/// The options of one pipeline run.
#[derive(Clone, Debug, Default)]
pub struct PassOptions {
    /// Override values by name or id.
    pub overrides: BTreeMap<String, String>,
    /// Leaves overrides for the backend to declare (section 17.3).
    pub keep_overrides: bool,
    pub bounds: Bounds,
    pub integer_division: IntegerDivision,
}

/// One unit a backend compiles: the module and the index of the entry
/// point it was selected for (`None` for a target that keeps every entry).
#[derive(Clone, Debug)]
pub struct Unit {
    pub module: Module,
    pub entry: Option<usize>,
}

/// The capabilities `target` cannot express (section 16.6).
#[must_use]
pub const fn unsupported_capabilities(target: Target) -> &'static [Capability] {
    match target {
        Target::Msl => &[Capability::Float64],
        Target::Glsl => &[
            Capability::Float16,
            Capability::Int64,
            Capability::Int64Atomics,
        ],
        Target::Wgsl => &[
            Capability::Float64,
            Capability::Int64,
            Capability::Int64Atomics,
            Capability::ImageAtomics,
        ],
        Target::Ksl => &[
            Capability::Float64,
            Capability::Int64,
            Capability::Int64Atomics,
            Capability::ImageAtomics,
            Capability::CubeArray,
            Capability::Multisampled,
            Capability::SampleShading,
            Capability::ClipDistances,
            Capability::DualSourceBlending,
        ],
        Target::Spirv | Target::Hlsl => &[],
    }
}

/// Runs steps 3 to 8 of section 17.1 on a normalized, validated module.
///
/// # Errors
/// Returns `shader.capability` for each capability of the module `target`
/// cannot express (section 16.6); otherwise the override problems of step
/// 4, among them `shader.override-unsupported` for kept overrides on a
/// target that cannot specialize (section 14.3).
pub fn run(
    module: &Module,
    target: Target,
    options: &PassOptions,
) -> Result<(Vec<Unit>, Vec<Baked>), Vec<ValidationError>> {
    let demoted;
    let module = if unsupported_capabilities(target).contains(&Capability::Float16)
        && halves::uses_halves(module)
    {
        let mut copy = module.clone();
        halves::demote_halves(&mut copy);
        demoted = copy;
        &demoted
    } else {
        module
    };
    let refused: Vec<ValidationError> = module
        .capabilities
        .iter()
        .filter(|capability| unsupported_capabilities(target).contains(capability))
        .map(|capability| {
            ValidationError::keyed(
                "capability",
                "shader.capability",
                format!("{} has no `{}`", target.name(), capability.name()),
                Span::default(),
            )
        })
        .collect();
    if !refused.is_empty() {
        return Err(refused);
    }
    // KSL is a source language: its options stay options, and the guards
    // and zeroing below are added again when the KSL is compiled.
    let source = target == Target::Ksl;
    let mut baked_module = module.clone();
    let baked = if options.keep_overrides || source {
        if matches!(target, Target::Hlsl | Target::Glsl) && !module.overrides.is_empty() {
            return Err(module
                .overrides
                .iter()
                .map(|(handle, item)| {
                    ValidationError::keyed(
                        "SV22",
                        "shader.override-unsupported",
                        format!("{} cannot keep override `{}`", target.name(), item.name),
                        module.overrides.span(handle),
                    )
                })
                .collect());
        }
        overrides::defaults(module, &options.overrides)?
    } else {
        overrides::bake(&mut baked_module, &options.overrides)?
    };
    let selected: Vec<(Module, Option<usize>)> = if target.per_stage() {
        (0..module.entry_points.len())
            .map(|entry| (select::select_entry(&baked_module, entry), Some(entry)))
            .collect()
    } else {
        vec![(baked_module, None)]
    };
    let mut units = Vec::new();
    for (mut unit, entry) in selected {
        if !source {
            bounds::guard_bounds(&mut unit, options.bounds);
        }
        if !matches!(target, Target::Wgsl | Target::Ksl)
            && options.integer_division == IntegerDivision::Defined
        {
            guards::guard_integers(&mut unit);
        }
        if !matches!(target, Target::Wgsl | Target::Ksl) {
            let used: Vec<_> = (0..unit.entry_points.len())
                .map(|index| compact::entry_globals(&unit, index))
                .collect();
            zeroing::zero_workgroup(&mut unit, &used);
        }
        compact::compact(&mut unit);
        units.push(Unit {
            module: unit,
            entry,
        });
    }
    Ok((units, baked))
}
