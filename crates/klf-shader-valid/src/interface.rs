//! Stage interface and shader rules: SV13, SV16 (interface), SV17, SV18,
//! SV20.

use crate::ValidationError;
use klf_shader::{
    Binding, BuiltIn, EntryPoint, Handle, Interpolation, Module, Sampling, Scalar, ScalarKind,
    ShaderKind, Stage, Type, TypeInner, WorkgroupExtent,
};
use std::collections::{BTreeMap, BTreeSet};

/// One bound value of an entry point's interface.
#[derive(Clone, Copy)]
pub(crate) struct Leaf {
    pub ty: Handle<Type>,
    pub binding: Binding,
    pub output: bool,
}

/// The bound leaves of an entry point; `Err` names a leaf without a
/// binding.
pub(crate) fn leaves(module: &Module, entry: &EntryPoint) -> Result<Vec<Leaf>, String> {
    let mut found = Vec::new();
    let mut add = |ty: Handle<Type>,
                   binding: Option<Binding>,
                   output: bool,
                   what: &str|
     -> Result<(), String> {
        if let Some(binding) = binding {
            found.push(Leaf {
                ty,
                binding,
                output,
            });
            return Ok(());
        }
        match &module.types[ty].inner {
            TypeInner::Struct { members, .. } => {
                for member in members {
                    let binding = member.binding.ok_or_else(|| {
                        format!("member `{}` of {what} has no binding", member.name)
                    })?;
                    found.push(Leaf {
                        ty: member.ty,
                        binding,
                        output,
                    });
                }
                Ok(())
            }
            _ => Err(format!("{what} has no binding")),
        }
    };
    for (index, argument) in entry.function.arguments.iter().enumerate() {
        add(
            argument.ty,
            argument.binding,
            false,
            &format!("argument {index}"),
        )?;
    }
    if let Some(result) = &entry.function.result {
        add(result.ty, result.binding, true, "the result")?;
    }
    Ok(found)
}

fn built_in_rule(built_in: BuiltIn, stage: Stage, output: bool) -> Option<TypeInner> {
    let u32_ = TypeInner::Scalar(Scalar::U32);
    let uvec3 = TypeInner::Vector {
        size: 3,
        scalar: Scalar::U32,
    };
    let (stages, ty): (&[(Stage, bool)], TypeInner) = match built_in {
        BuiltIn::Position => (
            &[(Stage::Vertex, true), (Stage::Fragment, false)],
            TypeInner::Vector {
                size: 4,
                scalar: Scalar::F32,
            },
        ),
        BuiltIn::VertexIndex | BuiltIn::InstanceIndex => (&[(Stage::Vertex, false)], u32_),
        BuiltIn::FrontFacing => (&[(Stage::Fragment, false)], TypeInner::Scalar(Scalar::BOOL)),
        BuiltIn::FragDepth => (&[(Stage::Fragment, true)], TypeInner::Scalar(Scalar::F32)),
        BuiltIn::SampleIndex => (&[(Stage::Fragment, false)], u32_),
        BuiltIn::SampleMask => (&[(Stage::Fragment, false), (Stage::Fragment, true)], u32_),
        BuiltIn::LocalInvocationIndex => (&[(Stage::Compute, false)], u32_),
        BuiltIn::GlobalInvocationId
        | BuiltIn::LocalInvocationId
        | BuiltIn::WorkgroupId
        | BuiltIn::NumWorkgroups => (&[(Stage::Compute, false)], uvec3),
        BuiltIn::ClipDistances => {
            return (stage == Stage::Vertex && output).then_some(TypeInner::Scalar(Scalar::F32));
        }
    };
    stages.contains(&(stage, output)).then_some(ty)
}

/// The interpolation and sampling a location binding means, defaults
/// applied (section 10.5 rule 2).
pub(crate) fn effective(module: &Module, leaf: &Leaf) -> (Option<u32>, Interpolation, Sampling) {
    let Binding::Location {
        location,
        interpolation,
        sampling,
        ..
    } = leaf.binding
    else {
        return (None, Interpolation::Flat, Sampling::Center);
    };
    let integer = module.types[leaf.ty]
        .inner
        .scalar()
        .is_some_and(|scalar| scalar.kind != ScalarKind::Float);
    let interpolation = interpolation.unwrap_or(if integer {
        Interpolation::Flat
    } else {
        Interpolation::Perspective
    });
    (
        location,
        interpolation,
        sampling.unwrap_or(Sampling::Center),
    )
}

fn check_entry(module: &Module, entry: &EntryPoint, problems: &mut Vec<ValidationError>) {
    let mut fail = |rule: &'static str, message: String| {
        problems.push(ValidationError::new(
            rule,
            format!("entry `{}`: {message}", entry.name),
            entry.span,
        ));
    };
    match (entry.stage, &entry.workgroup_size) {
        (Stage::Compute, None) => fail(
            "SV20",
            "a compute entry declares a workgroup size".to_owned(),
        ),
        (Stage::Compute, Some(size)) => {
            if size.contains(&WorkgroupExtent::Fixed(0)) {
                fail("SV20", "each workgroup extent is at least 1".to_owned());
            }
            if entry.function.result.is_some() {
                fail("SV20", "a compute entry has no result".to_owned());
            }
        }
        (_, Some(_)) => fail(
            "SV20",
            "only compute entries declare a workgroup size".to_owned(),
        ),
        (_, None) => {}
    }
    let leaves = match leaves(module, entry) {
        Ok(leaves) => leaves,
        Err(message) => {
            fail("SV17", message);
            return;
        }
    };
    let mut locations = BTreeSet::new();
    let mut built_ins = BTreeSet::new();
    let mut writes_position = false;
    for leaf in &leaves {
        let inner = &module.types[leaf.ty].inner;
        match leaf.binding {
            Binding::BuiltIn(built_in) => {
                if !built_ins.insert((built_in.name(), leaf.output)) {
                    fail(
                        "SV17",
                        format!("built-in `{}` is bound twice", built_in.name()),
                    );
                }
                writes_position |= built_in == BuiltIn::Position && leaf.output;
                match built_in_rule(built_in, entry.stage, leaf.output) {
                    None => fail(
                        "SV17",
                        format!("built-in `{}` is not available here", built_in.name()),
                    ),
                    Some(TypeInner::Scalar(Scalar::F32)) if built_in == BuiltIn::ClipDistances => {
                        if !matches!(inner, TypeInner::Array { .. }) {
                            fail("SV17", "clip_distances is an array of f32".to_owned());
                        }
                    }
                    Some(wanted) if wanted != *inner => {
                        fail(
                            "SV17",
                            format!("built-in `{}` has the wrong type", built_in.name()),
                        );
                    }
                    Some(_) => {}
                }
            }
            Binding::Location {
                location,
                blend_src,
                ..
            } => {
                if entry.stage == Stage::Compute {
                    fail(
                        "SV20",
                        "a compute entry's arguments are built-ins only".to_owned(),
                    );
                }
                let Some((_, scalar)) = inner.scalar_or_vector() else {
                    fail("SV17", "a location carries a scalar or vector".to_owned());
                    continue;
                };
                if scalar.kind == ScalarKind::Bool {
                    fail("SV16", "a bool crosses the stage interface".to_owned());
                }
                let Some(location) = location else {
                    fail("SV17", "a location is still `auto`".to_owned());
                    continue;
                };
                if !locations.insert((location, leaf.output, blend_src)) {
                    fail("SV17", format!("location {location} is bound twice"));
                }
                let (_, interpolation, _) = effective(module, leaf);
                if scalar.kind != ScalarKind::Float && interpolation != Interpolation::Flat {
                    fail("SV17", format!("integer location {location} is flat"));
                }
                if let Some(source) = blend_src
                    && (source > 1
                        || location != 0
                        || !leaf.output
                        || entry.stage != Stage::Fragment)
                {
                    fail(
                        "SV17",
                        "blend_src is 0 or 1 on fragment output location 0".to_owned(),
                    );
                }
            }
        }
    }
    if entry.stage == Stage::Vertex && !writes_position {
        fail(
            "SV17",
            "a vertex entry writes the position built-in".to_owned(),
        );
    }
}

/// SV16, SV17 and SV20 for every entry point.
pub(crate) fn check_interfaces(module: &Module, problems: &mut Vec<ValidationError>) {
    let mut names = BTreeSet::new();
    for entry in &module.entry_points {
        if !names.insert(entry.name.as_str()) {
            problems.push(ValidationError::new(
                "SV19",
                format!("two entry points are named `{}`", entry.name),
                entry.span,
            ));
        }
        check_entry(module, entry, problems);
    }
}

fn check_stage_match(
    module: &Module,
    vertex: &EntryPoint,
    fragment: &EntryPoint,
    problems: &mut Vec<ValidationError>,
) {
    let (Ok(outputs), Ok(inputs)) = (leaves(module, vertex), leaves(module, fragment)) else {
        return;
    };
    let produced: BTreeMap<u32, (&Leaf, Interpolation, Sampling)> = outputs
        .iter()
        .filter(|leaf| leaf.output)
        .filter_map(|leaf| {
            let (location, interpolation, sampling) = effective(module, leaf);
            Some((location?, (leaf, interpolation, sampling)))
        })
        .collect();
    for input in inputs.iter().filter(|leaf| !leaf.output) {
        let (Some(location), interpolation, sampling) = effective(module, input) else {
            continue;
        };
        let matches =
            produced
                .get(&location)
                .is_some_and(|(output, out_interpolation, out_sampling)| {
                    module.types[output.ty].inner == module.types[input.ty].inner
                        && *out_interpolation == interpolation
                        && *out_sampling == sampling
                });
        if !matches {
            problems.push(ValidationError::keyed(
                "SV18",
                "shader.interface-mismatch",
                format!(
                    "fragment input location {location} of `{}` is not produced alike by `{}`",
                    fragment.name, vertex.name
                ),
                fragment.span,
            ));
        }
    }
}

/// SV13 and SV18 for every shader declaration.
pub(crate) fn check_shaders(module: &Module, problems: &mut Vec<ValidationError>) {
    for shader in &module.shaders {
        let mut fail = |rule: &'static str, message: String| {
            problems.push(ValidationError::new(
                rule,
                format!("shader `{}`: {message}", shader.name),
                shader.span,
            ));
        };
        let mut stages = Vec::new();
        for entry in &shader.entries {
            match module.entry_points.get(*entry) {
                Some(found) => stages.push((found.stage, found)),
                None => fail("SV1", format!("entry {entry} does not exist")),
            }
        }
        let wanted: &[Stage] = match shader.kind {
            ShaderKind::Graphics => &[Stage::Vertex, Stage::Fragment],
            ShaderKind::Compute => &[Stage::Compute],
        };
        let found: Vec<Stage> = stages.iter().map(|(stage, _)| *stage).collect();
        // A graphics shader may lack either stage, as a depth-only pass
        // lacks a fragment one, and so may a unit stage selection made.
        let fits = !found.is_empty()
            && found.iter().all(|stage| wanted.contains(stage))
            && (0..found.len()).all(|at| !found[..at].contains(&found[at]));
        if !fits {
            fail(
                "SV20",
                format!(
                    "a {} shader has entries {wanted:?}, not {found:?}",
                    shader.kind.name()
                ),
            );
        }
        let mut members = BTreeSet::new();
        let mut bindings = BTreeSet::new();
        for group in &shader.groups {
            for global in &group.globals {
                let Some(variable) = module.globals.get(*global) else {
                    fail(
                        "SV1",
                        "a group names a global that does not exist".to_owned(),
                    );
                    continue;
                };
                if !variable.space.is_resource() {
                    fail(
                        "SV13",
                        format!("`{}` is in a group but is not a resource", variable.name),
                    );
                }
                if !members.insert(*global) {
                    fail("SV13", format!("`{}` belongs to two groups", variable.name));
                }
                match variable.binding {
                    None => fail(
                        "SV13",
                        format!("resource `{}` has no binding", variable.name),
                    ),
                    Some(binding) if !bindings.insert(binding) => fail(
                        "SV13",
                        format!(
                            "two resources share binding ({}, {})",
                            binding.group, binding.binding
                        ),
                    ),
                    Some(_) => {}
                }
            }
        }
        let of = |wanted: Stage| {
            stages
                .iter()
                .find(|(stage, _)| *stage == wanted)
                .map(|(_, entry)| *entry)
        };
        if let (Some(vertex), Some(fragment)) = (of(Stage::Vertex), of(Stage::Fragment)) {
            check_stage_match(module, vertex, fragment, problems);
        }
    }
}
