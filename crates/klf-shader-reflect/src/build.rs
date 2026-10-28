//! Building the reflection of a shader from its module, validation
//! information and binding assignment (section 21.1).

use crate::model::{
    BindRecord, FieldRecord, Layout, LayoutField, OptionRecord, Reflection, ResourceRecord,
    StageRecord, TypeRecord,
};
use klf_shader::{
    AddressSpace, Binding, BuiltIn, Handle, ImageClass, ImageDim, Literal, Module, ScalarKind,
    Target, Type, TypeInner, WorkgroupExtent,
};
use klf_shader_bind::{Assignment, ResourceKind};
use klf_shader_passes::Baked;
use klf_shader_valid::{ConstValue, Evaluator, ModuleInfo};
use std::collections::HashMap;

/// How a type is spelled in reflection: the KSL spelling the oracle's
/// hosts read.
#[must_use]
pub fn type_name(module: &Module, ty: Handle<Type>) -> String {
    let declared = &module.types[ty];
    let scalar = |scalar: klf_shader::Scalar| match scalar.kind {
        ScalarKind::Bool => "Bool",
        ScalarKind::Sint => "Int",
        ScalarKind::Uint => "UInt",
        ScalarKind::Float => "Float",
    };
    match &declared.inner {
        TypeInner::Scalar(value) | TypeInner::Atomic(value) => scalar(*value).to_owned(),
        TypeInner::Vector {
            size,
            scalar: value,
        } => format!("{}{size}", scalar(*value)),
        TypeInner::Matrix { columns, rows, .. } => format!("Float{columns}x{rows}"),
        TypeInner::Struct { .. } => declared.name.clone().unwrap_or_else(|| "Struct".to_owned()),
        TypeInner::Array { base, .. } => format!("[{}]", type_name(module, *base)),
        TypeInner::Image { dim, class, .. } => match (dim, class) {
            (ImageDim::Cube, _) => "TextureCube",
            (_, ImageClass::Depth { .. }) => "Depth2d",
            (
                _,
                ImageClass::Sampled {
                    kind: ScalarKind::Uint,
                    ..
                },
            ) => "Texture2dUint",
            _ => "Texture2d",
        }
        .to_owned(),
        TypeInner::Sampler { comparison: true } => "SamplerComparison".to_owned(),
        TypeInner::Sampler { comparison: false } => "Sampler".to_owned(),
        TypeInner::Pointer { base, .. } => type_name(module, *base),
    }
}

/// The KSL word of a built-in.
#[must_use]
pub const fn builtin_word(built_in: BuiltIn) -> &'static str {
    match built_in {
        BuiltIn::Position => "position",
        BuiltIn::VertexIndex => "vertex_index",
        BuiltIn::InstanceIndex => "instance_index",
        BuiltIn::FrontFacing => "front_facing",
        BuiltIn::GlobalInvocationId => "thread_id",
        BuiltIn::LocalInvocationId => "local_id",
        BuiltIn::WorkgroupId => "group_id",
        BuiltIn::LocalInvocationIndex => "local_index",
        BuiltIn::FragDepth => "frag_depth",
        BuiltIn::SampleIndex => "sample_index",
        BuiltIn::SampleMask => "sample_mask",
        BuiltIn::NumWorkgroups => "num_groups",
        BuiltIn::ClipDistances => "clip_distances",
    }
}

/// How a constant is written in reflection.
#[must_use]
pub fn literal_text(literal: Literal) -> String {
    match literal {
        Literal::Bool(value) => value.to_string(),
        Literal::I32(value) => value.to_string(),
        Literal::U32(value) => value.to_string(),
        Literal::I64(value) => value.to_string(),
        Literal::U64(value) => value.to_string(),
        Literal::F16(bits) => format!("{:?}", klf_shader::print::half_to_f32(bits)),
        Literal::F32(bits) => format!("{:?}", f32::from_bits(bits)),
        Literal::F64(bits) => format!("{:?}", f64::from_bits(bits)),
    }
}

fn field(module: &Module, member: &klf_shader::StructMember, location: bool) -> FieldRecord {
    let (builtin, interpolation, at) = match member.binding {
        Some(Binding::BuiltIn(built_in)) => (Some(builtin_word(built_in).to_owned()), None, None),
        Some(Binding::Location {
            location: at,
            interpolation,
            ..
        }) => (None, interpolation.map(|value| value.name().to_owned()), at),
        None => (None, None, None),
    };
    FieldRecord {
        name: member.name.clone(),
        type_name: type_name(module, member.ty),
        builtin,
        interpolation,
        location: if location { at } else { None },
    }
}

fn struct_fields(module: &Module, ty: Option<Handle<Type>>, location: bool) -> Vec<FieldRecord> {
    match ty.map(|ty| &module.types[ty].inner) {
        Some(TypeInner::Struct { members, .. }) => members
            .iter()
            .map(|member| field(module, member, location))
            .collect(),
        _ => Vec::new(),
    }
}

/// The uniform layout of a struct: its members as recorded, the alignment
/// raised to 16 and the size rounded to it (section 21.1).
fn layout(module: &Module, ty: Handle<Type>) -> Option<Layout> {
    let TypeInner::Struct { members, align, .. } = &module.types[ty].inner else {
        return None;
    };
    let alignment = (*align).max(16);
    let end = members
        .iter()
        .map(|member| member.offset + member.size)
        .max()
        .unwrap_or(0);
    Some(Layout {
        alignment,
        size: end.div_ceil(alignment) * alignment,
        fields: members
            .iter()
            .map(|member| LayoutField {
                name: member.name.clone(),
                offset: member.offset,
                alignment: member.align,
                size: member.size,
                stride: member.size.div_ceil(member.align.max(1)) * member.align.max(1),
            })
            .collect(),
    })
}

fn struct_named(module: &Module, ty: Handle<Type>) -> Option<(String, Handle<Type>)> {
    match &module.types[ty].inner {
        TypeInner::Struct { .. } => module.types[ty].name.clone().map(|name| (name, ty)),
        TypeInner::Array { base, .. } => struct_named(module, *base),
        _ => None,
    }
}

fn access(module: &Module, global: &klf_shader::GlobalVariable) -> Option<String> {
    let access = match (global.space, &module.types[global.ty].inner) {
        (AddressSpace::Storage { access }, _) => access,
        (
            _,
            TypeInner::Image {
                class: ImageClass::Storage { access, .. },
                ..
            },
        ) => *access,
        _ => return None,
    };
    Some(
        match (access.load, access.store || access.atomic) {
            (true, true) => "read_write",
            (false, true) => "write",
            _ => "read",
        }
        .to_owned(),
    )
}

/// What reflection needs beyond the module.
pub struct Context<'a> {
    pub module: &'a Module,
    pub info: &'a ModuleInfo,
    pub shader: usize,
    pub assignment: &'a Assignment,
    pub target: Target,
    pub baked: &'a [Baked],
    pub keep_overrides: bool,
}

/// The reflection of one shader for one target.
#[must_use]
pub fn reflect(context: &Context<'_>) -> Reflection {
    let module = context.module;
    let shader = &module.shaders[context.shader];
    let mut named: Vec<(String, Handle<Type>)> = Vec::new();
    let mut stages = Vec::new();
    for entry_index in &shader.entries {
        let entry = &module.entry_points[*entry_index];
        let input = entry.function.arguments.first().map(|argument| argument.ty);
        let output = entry.function.result.as_ref().map(|result| result.ty);
        let input_named = input.and_then(|ty| struct_named(module, ty));
        let output_named = output.and_then(|ty| struct_named(module, ty));
        named.extend(input_named.clone());
        named.extend(output_named.clone());
        stages.push(StageRecord {
            stage: entry.stage,
            entry_name: entry.name.clone(),
            input_type: input_named.map(|(name, _)| name),
            output_type: output_named.map(|(name, _)| name),
            threads: entry.workgroup_size.map(|size| {
                size.map(|extent| match extent {
                    WorkgroupExtent::Fixed(value) => value,
                    WorkgroupExtent::Override(_) => 0,
                })
            }),
            inputs: struct_fields(module, input, true),
            outputs: struct_fields(module, output, true),
        });
    }
    let mut resources = Vec::new();
    for resource in &context.assignment.resources {
        let global = &module.globals[resource.global];
        let group = &shader.groups[resource.group];
        if matches!(resource.kind, ResourceKind::Uniform | ResourceKind::Storage) {
            named.extend(struct_named(module, global.ty));
        }
        let visibility = shader
            .entries
            .iter()
            .filter(|entry| {
                context.info.entry_globals(module, **entry)[resource.global.index()].any()
            })
            .map(|entry| module.entry_points[*entry].stage)
            .collect();
        resources.push(ResourceRecord {
            group_name: group.name.clone(),
            group_class: group.class.name().to_owned(),
            group_index: resource.group as u32,
            name: global.name.clone(),
            kind: resource.kind.name().to_owned(),
            type_name: type_name(module, global.ty),
            access: access(module, global),
            visibility,
            bindings: resource
                .slots
                .iter()
                .map(|slot| BindRecord {
                    target: slot.target,
                    group: slot.group,
                    binding: slot.binding,
                    glsl_name: slot.glsl_name.clone(),
                })
                .collect(),
            paired_sampler: resource
                .paired_sampler
                .map(|sampler| module.globals[sampler].name.clone()),
            lengths: resource
                .msl_length
                .map(|k| (Target::Msl, k))
                .into_iter()
                .collect(),
        });
    }
    named.sort_by(|left, right| left.0.cmp(&right.0));
    named.dedup_by(|left, right| left.0 == right.0);
    Reflection {
        shader_name: shader.name.clone(),
        shader_kind: shader.kind.name().to_owned(),
        target: Some(context.target),
        source: module.lang.clone(),
        options: options(context),
        stages,
        resources,
        types: named
            .into_iter()
            .map(|(name, ty)| TypeRecord {
                name,
                fields: struct_fields(module, Some(ty), false),
                layout: layout(module, ty),
            })
            .collect(),
        renames: Vec::new(),
    }
}

fn options(context: &Context<'_>) -> Vec<OptionRecord> {
    let module = context.module;
    let shader = &module.shaders[context.shader];
    let mut values = HashMap::new();
    let mut records = Vec::new();
    for item in &shader.options {
        let declared = &module.overrides[*item];
        let value = match context
            .baked
            .iter()
            .find(|baked| baked.name == declared.name)
        {
            Some(baked) => Some(baked.value),
            None => declared
                .init
                .and_then(|init| Evaluator::new(module, values.clone()).eval(init).ok())
                .and_then(|value| value.scalar())
                .map(klf_shader_valid::ConstScalar::to_literal),
        };
        if let Some(value) = value {
            values.insert(
                *item,
                ConstValue::Scalar(klf_shader_valid::ConstScalar::from_literal(value)),
            );
        }
        records.push(OptionRecord {
            name: declared.name.clone(),
            type_name: type_name(module, declared.ty),
            value: value.map_or_else(|| "-".to_owned(), literal_text),
            id: declared.id,
            keep: context.keep_overrides,
        });
    }
    records
}
