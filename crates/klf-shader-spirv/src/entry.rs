//! The entry point interface: one `Input` or `Output` variable per bound
//! leaf, loaded into the arguments on entry and stored from the result on
//! return (section 19.4 rules 2 and 3).

use crate::Writer;
use crate::function::FunctionWriter;
use crate::words::{capability, class, decoration, instruction, op};
use klf_shader::{
    Binding, BuiltIn, EarlyDepthTest, EntryPoint, Handle, Interpolation, Literal, Sampling, Scalar,
    ScalarKind, Stage, Type, TypeInner, WorkgroupExtent,
};

/// Where one result leaf is stored.
pub(crate) struct Output {
    variable: u32,
    member: Option<u32>,
    /// Whether the variable is SPIR-V's `SampleMask` array.
    mask: bool,
}

fn built_in_word(built_in: BuiltIn, stage: Stage, output: bool) -> u32 {
    match built_in {
        BuiltIn::Position if stage == Stage::Fragment && !output => 15,
        BuiltIn::Position => 0,
        BuiltIn::VertexIndex => 42,
        BuiltIn::InstanceIndex => 43,
        BuiltIn::FrontFacing => 17,
        BuiltIn::FragDepth => 22,
        BuiltIn::SampleIndex => 18,
        BuiltIn::SampleMask => 20,
        BuiltIn::GlobalInvocationId => 28,
        BuiltIn::LocalInvocationId => 27,
        BuiltIn::LocalInvocationIndex => 29,
        BuiltIn::WorkgroupId => 26,
        BuiltIn::NumWorkgroups => 24,
        BuiltIn::ClipDistances => 3,
    }
}

/// Declares one interface variable; returns it and whether it is the
/// `SampleMask` array.
fn variable(
    writer: &mut FunctionWriter<'_, '_>,
    ty: Handle<Type>,
    binding: Binding,
    output: bool,
    name: &str,
    stage: Stage,
) -> (u32, bool) {
    let w = &mut *writer.w;
    let module = w.module;
    let mask = binding == Binding::BuiltIn(BuiltIn::SampleMask);
    let value_type = w.type_of(ty, false);
    let variable_type = if mask {
        let length = w.literal(Literal::U32(1));
        let key = crate::types::TypeKey::Aggregate(
            Some("_ksir_sample_mask".to_owned()),
            TypeInner::Scalar(Scalar::U32),
            false,
        );
        if let Some(id) = w.types.get(&key) {
            *id
        } else {
            let id = w.id();
            w.declare(op::TYPE_ARRAY, &[id, value_type, length]);
            w.types.insert(key, id);
            id
        }
    } else {
        value_type
    };
    let storage = if output { class::OUTPUT } else { class::INPUT };
    let pointer = w.pointer(storage, variable_type);
    let id = w.id();
    w.declare(op::VARIABLE, &[pointer, id, storage]);
    w.name(id, name);
    w.interface.push(id);
    match binding {
        Binding::BuiltIn(built_in) => {
            match built_in {
                BuiltIn::ClipDistances => {
                    w.capabilities.insert(capability::CLIP_DISTANCE);
                }
                BuiltIn::SampleIndex => {
                    w.capabilities.insert(capability::SAMPLE_RATE_SHADING);
                }
                _ => {}
            }
            w.decorate(
                id,
                decoration::BUILT_IN,
                &[built_in_word(built_in, stage, output)],
            );
            if stage == Stage::Fragment
                && !output
                && module.types[ty]
                    .inner
                    .scalar()
                    .is_some_and(|scalar| scalar.kind.is_integer())
            {
                w.decorate(id, decoration::FLAT, &[]);
            }
        }
        Binding::Location {
            location,
            interpolation,
            sampling,
            blend_src,
        } => {
            w.decorate(id, decoration::LOCATION, &[location.unwrap_or(0)]);
            if let Some(source) = blend_src {
                w.decorate(id, decoration::INDEX, &[source]);
            }
            let interpolated =
                (stage == Stage::Fragment && !output) || (stage == Stage::Vertex && output);
            if interpolated {
                let integer = module.types[ty]
                    .inner
                    .scalar()
                    .is_some_and(|scalar| scalar.kind != ScalarKind::Float);
                let interpolation = interpolation.unwrap_or(if integer {
                    Interpolation::Flat
                } else {
                    Interpolation::Perspective
                });
                match interpolation {
                    Interpolation::Flat => w.decorate(id, decoration::FLAT, &[]),
                    Interpolation::Linear => w.decorate(id, decoration::NO_PERSPECTIVE, &[]),
                    Interpolation::Perspective => {}
                }
                match sampling {
                    Some(Sampling::Centroid) => w.decorate(id, decoration::CENTROID, &[]),
                    Some(Sampling::Sample) => {
                        w.capabilities.insert(capability::SAMPLE_RATE_SHADING);
                        w.decorate(id, decoration::SAMPLE, &[]);
                    }
                    _ => {}
                }
            }
        }
    }
    (id, mask)
}

/// Loads every input into the entry's arguments and declares the outputs.
pub(crate) fn prologue(writer: &mut FunctionWriter<'_, '_>, entry: &EntryPoint) {
    let module = writer.w.module;
    for (index, argument) in entry.function.arguments.iter().enumerate() {
        let argument_name = argument
            .name
            .clone()
            .unwrap_or_else(|| format!("_ksir_in{index}"));
        let value = if let Some(binding) = argument.binding {
            load(writer, argument.ty, binding, &argument_name, entry.stage)
        } else if let TypeInner::Struct { members, .. } = &module.types[argument.ty].inner {
            let mut parts = Vec::new();
            for member in members {
                let binding = member.binding.unwrap_or_else(Binding::auto);
                let name = format!("{argument_name}_{}", member.name);
                parts.push(load(writer, member.ty, binding, &name, entry.stage));
            }
            let ty = writer.w.type_of(argument.ty, false);
            writer.result(op::COMPOSITE_CONSTRUCT, ty, &parts)
        } else {
            0
        };
        writer.arguments.push(value);
    }
    if let Some(result) = &entry.function.result {
        if let Some(binding) = result.binding {
            let (id, mask) = variable(writer, result.ty, binding, true, "_ksir_out", entry.stage);
            writer.outputs.push(Output {
                variable: id,
                member: None,
                mask,
            });
        } else if let TypeInner::Struct { members, .. } = &module.types[result.ty].inner {
            for (index, member) in members.iter().enumerate() {
                let binding = member.binding.unwrap_or_else(Binding::auto);
                let name = format!("_ksir_out_{}", member.name);
                let (id, mask) = variable(writer, member.ty, binding, true, &name, entry.stage);
                writer.outputs.push(Output {
                    variable: id,
                    member: Some(index as u32),
                    mask,
                });
            }
        }
    }
}

fn load(
    writer: &mut FunctionWriter<'_, '_>,
    ty: Handle<Type>,
    binding: Binding,
    name: &str,
    stage: Stage,
) -> u32 {
    let (id, mask) = variable(writer, ty, binding, false, name, stage);
    let value_type = writer.w.type_of(ty, false);
    if mask {
        let pointer = writer.w.pointer(class::INPUT, value_type);
        let zero = writer.w.literal(Literal::U32(0));
        let element = writer.result(op::ACCESS_CHAIN, pointer, &[id, zero]);
        return writer.result(op::LOAD, value_type, &[element]);
    }
    writer.result(op::LOAD, value_type, &[id])
}

/// Stores a returned value into the outputs.
pub(crate) fn store_result(writer: &mut FunctionWriter<'_, '_>, value: u32) {
    let module = writer.w.module;
    let Some(entry) = writer.entry else {
        return;
    };
    let Some(result) = &entry.function.result else {
        return;
    };
    let member_types: Vec<Handle<Type>> = match &module.types[result.ty].inner {
        TypeInner::Struct { members, .. } => members.iter().map(|member| member.ty).collect(),
        _ => Vec::new(),
    };
    let targets: Vec<(u32, Option<u32>, bool)> = writer
        .outputs
        .iter()
        .map(|output| (output.variable, output.member, output.mask))
        .collect();
    for (variable, member, mask) in targets {
        let part = match member {
            Some(index) => {
                let ty = writer.w.type_of(member_types[index as usize], false);
                writer.result(op::COMPOSITE_EXTRACT, ty, &[value, index])
            }
            None => value,
        };
        if mask {
            let ty = writer.w.scalar(Scalar::U32);
            let pointer = writer.w.pointer(class::OUTPUT, ty);
            let zero = writer.w.literal(Literal::U32(0));
            let element = writer.result(op::ACCESS_CHAIN, pointer, &[variable, zero]);
            writer.emit(op::STORE, &[element, part]);
        } else {
            writer.emit(op::STORE, &[variable, part]);
        }
    }
}

/// The execution modes of the unit's entry point.
pub(crate) fn execution_modes(writer: &mut Writer<'_>, entry_id: u32) -> Vec<u32> {
    let module = writer.module;
    let entry = &module.entry_points[0];
    let mut words = Vec::new();
    match entry.stage {
        Stage::Vertex => {}
        Stage::Fragment => {
            words.extend(instruction(op::EXECUTION_MODE, &[entry_id, 7]));
            let writes_depth = entry.function.result.as_ref().is_some_and(|result| {
                result.binding == Some(Binding::BuiltIn(BuiltIn::FragDepth))
                    || matches!(&module.types[result.ty].inner, TypeInner::Struct { members, .. }
                        if members.iter().any(|member| member.binding == Some(Binding::BuiltIn(BuiltIn::FragDepth))))
            });
            if writes_depth {
                words.extend(instruction(op::EXECUTION_MODE, &[entry_id, 12]));
            }
            let early = match entry.early_depth_test {
                Some(EarlyDepthTest::Force) => Some(9),
                Some(EarlyDepthTest::GreaterEqual) => Some(14),
                Some(EarlyDepthTest::LessEqual) => Some(15),
                Some(EarlyDepthTest::Unchanged) => Some(16),
                None => None,
            };
            if let Some(mode) = early {
                words.extend(instruction(op::EXECUTION_MODE, &[entry_id, mode]));
            }
        }
        Stage::Compute => {
            let extents = entry
                .workgroup_size
                .unwrap_or([WorkgroupExtent::Fixed(1); 3]);
            writer.workgroup_size(extents);
            let mut operands = vec![entry_id, 17];
            for extent in extents {
                match extent {
                    WorkgroupExtent::Fixed(value) => operands.push(value),
                    WorkgroupExtent::Override(_) => operands.push(1),
                }
            }
            words.extend(instruction(op::EXECUTION_MODE, &operands));
        }
    }
    words
}
