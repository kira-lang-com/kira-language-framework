//! The module-scope tables: the unit, its groups, types, members,
//! constants, overrides and globals, and the functions with their
//! arguments and locals.

use crate::record::Record;
use crate::values::value;
use klf_shader::print::literal_text;
use klf_shader::{
    AddressSpace, ArraySize, Binding, FunctionRef, ImageClass, LayoutRule, Module, ScalarKind,
    Target, TypeInner, WorkgroupExtent,
};
use klf_shader_bind::{Resource, hlsl_class};
use klf_shader_valid::{GlobalUse, ModuleInfo};
use std::collections::{BTreeSet, HashMap};

pub(crate) type Tables = HashMap<(&'static str, i64), Vec<Record>>;

pub(crate) fn build(
    module: &Module,
    info: &ModuleInfo,
    resources: &[Option<Resource>],
    target: Target,
    debug: bool,
    tables: &mut Tables,
) {
    tables.insert(("unit", -1), vec![unit(module, target, debug)]);
    tables.insert(("groups", -1), groups(module));
    types(module, tables);
    let mut values = Vec::new();
    tables.insert(
        ("constants", -1),
        module
            .constants
            .iter()
            .map(|(_, constant)| {
                let mut record = Record::new("constant");
                record
                    .text("name", &constant.name)
                    .int("type", constant.ty.index() as i64)
                    .int("init", constant.init.index() as i64);
                if let Some(at) = value(module, info, constant.init, constant.ty, &mut values) {
                    record.int("value", at);
                }
                record.finish()
            })
            .collect(),
    );
    tables.insert(
        ("overrides", -1),
        module
            .overrides
            .iter()
            .map(|(_, item)| {
                let mut record = Record::new("override");
                record
                    .text("name", &item.name)
                    .int("type", item.ty.index() as i64)
                    .handle("init", item.init);
                if let Some(id) = item.id {
                    record.int("id", id);
                }
                if let Some(at) = item
                    .init
                    .and_then(|init| value(module, info, init, item.ty, &mut values))
                {
                    record.int("value", at);
                }
                record.finish()
            })
            .collect(),
    );
    tables.insert(
        ("globals", -1),
        globals(module, info, resources, target, &mut values),
    );
    tables.insert(("values", -1), values);
    functions(module, info, tables);
}

/// The number section 6.7 gives a function: the arena's functions, then
/// the entry points.
pub(crate) fn number(module: &Module, which: FunctionRef) -> i64 {
    match which {
        FunctionRef::Function(handle) => handle.index() as i64,
        FunctionRef::Entry(index) => (module.functions.len() + index) as i64,
    }
}

fn unit(module: &Module, target: Target, debug: bool) -> Record {
    let shader = module.shaders.first();
    let mut record = Record::new(shader.map_or("library", |shader| shader.kind.name()));
    record
        .text("name", &module.name)
        .text("target", target.name())
        .text("lang", &module.lang)
        .flag("debug", debug)
        .texts(
            "capabilities",
            module
                .capabilities
                .iter()
                .map(|capability| capability.name().to_owned())
                .collect(),
        )
        .texts("files", module.files.clone())
        .ints(
            "entries",
            (0..module.entry_points.len())
                .map(|entry| number(module, FunctionRef::Entry(entry)))
                .collect(),
        );
    if let Some(shader) = shader {
        record.text("shader", &shader.name).ints(
            "options",
            shader
                .options
                .iter()
                .map(|item| item.index() as i64)
                .collect(),
        );
    }
    if let [entry] = module.entry_points.as_slice() {
        record.text("stage", entry.stage.name());
    }
    record.finish()
}

fn groups(module: &Module) -> Vec<Record> {
    let Some(shader) = module.shaders.first() else {
        return Vec::new();
    };
    shader
        .groups
        .iter()
        .map(|group| {
            Record::new(group.class.name())
                .text("name", &group.name)
                .ints(
                    "globals",
                    group
                        .globals
                        .iter()
                        .map(|global| global.index() as i64)
                        .collect(),
                )
                .finish()
        })
        .collect()
}

fn scalar_of_kind(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Bool => "bool",
        ScalarKind::Sint => "i32",
        ScalarKind::Uint => "u32",
        ScalarKind::Float => "f32",
    }
}

/// A space's word, its access apart.
fn space_word(space: AddressSpace) -> (&'static str, Option<String>) {
    match space {
        AddressSpace::Function => ("function", None),
        AddressSpace::Private => ("private", None),
        AddressSpace::Workgroup => ("workgroup", None),
        AddressSpace::Uniform => ("uniform", None),
        AddressSpace::Storage { access } => ("storage", Some(access.words().join(" "))),
        AddressSpace::Handle => ("handle", None),
        AddressSpace::Immediate => ("immediate", None),
    }
}

#[allow(clippy::too_many_lines)]
fn types(module: &Module, tables: &mut Tables) {
    let host = host_structs(module);
    let mut records = Vec::new();
    for (handle, ty) in module.types.iter() {
        let mut record = Record::new(match &ty.inner {
            TypeInner::Scalar(_) => "scalar",
            TypeInner::Vector { .. } => "vector",
            TypeInner::Matrix { .. } => "matrix",
            TypeInner::Atomic(_) => "atomic",
            TypeInner::Pointer { .. } => "pointer",
            TypeInner::Array { .. } => "array",
            TypeInner::Struct { .. } => "struct",
            TypeInner::Image { .. } => "image",
            TypeInner::Sampler { .. } => "sampler",
        });
        record.optional_text("name", ty.name.as_deref());
        if let Some(scalar) = ty.inner.scalar() {
            record.text("scalar", scalar.text());
        }
        if matches!(
            ty.inner,
            TypeInner::Scalar(_)
                | TypeInner::Vector { .. }
                | TypeInner::Matrix { .. }
                | TypeInner::Atomic(_)
        ) {
            for (rule, size, align) in [
                (LayoutRule::Std140, "size_std140", "align_std140"),
                (LayoutRule::Std430, "size_std430", "align_std430"),
                (LayoutRule::Msl, "size_msl", "align_msl"),
            ] {
                let (bytes, alignment) = klf_shader::layout::extent(&module.types, handle, rule);
                record.int(size, bytes).int(align, alignment);
            }
        }
        match &ty.inner {
            TypeInner::Scalar(_) | TypeInner::Atomic(_) => {}
            TypeInner::Vector { size, .. } => {
                record.int("lanes", *size);
            }
            TypeInner::Matrix { columns, rows, .. } => {
                record.int("columns", *columns).int("rows", *rows);
            }
            TypeInner::Pointer { base, space } => {
                let (word, access) = space_word(*space);
                record
                    .int("base", base.index() as i64)
                    .text("space", word)
                    .optional_text("access", access.as_deref());
            }
            TypeInner::Array { base, size, stride } => {
                record
                    .int("base", base.index() as i64)
                    .int("stride", *stride);
                match size {
                    ArraySize::Constant(count) => {
                        record
                            .int("length", *count)
                            .int("size", i64::from(*count) * i64::from(*stride));
                    }
                    ArraySize::Override(item) => {
                        record.int("length_override", item.index() as i64);
                    }
                    ArraySize::Runtime => {}
                }
            }
            TypeInner::Struct {
                rule,
                members,
                size,
                align,
            } => {
                record
                    .int("size", *size)
                    .int("align", *align)
                    .text("rule", rule.name())
                    .flag("host_shared", host[handle.index()]);
                tables.insert(
                    ("members", handle.index() as i64),
                    members
                        .iter()
                        .map(|member| {
                            let mut item = Record::new("member");
                            item.text("name", &member.name)
                                .int("type", member.ty.index() as i64)
                                .int("offset", member.offset)
                                .int("size", member.size)
                                .int("align", member.align);
                            binding(&mut item, member.binding.as_ref(), "");
                            item.finish()
                        })
                        .collect(),
                );
            }
            TypeInner::Image {
                dim,
                arrayed,
                class,
            } => {
                record
                    .text("dimension", dim.name())
                    .flag("arrayed", *arrayed);
                match class {
                    ImageClass::Sampled { kind, multisampled } => {
                        record
                            .text("class", "sampled")
                            .text("scalar", scalar_of_kind(*kind))
                            .flag("multisampled", *multisampled);
                    }
                    ImageClass::Depth { multisampled } => {
                        record
                            .text("class", "depth")
                            .flag("multisampled", *multisampled);
                    }
                    ImageClass::Storage { format, access } => {
                        record
                            .text("class", "storage")
                            .text("scalar", scalar_of_kind(format.kind()))
                            .text("format", format.name())
                            .text("access", access.words().join(" "));
                    }
                }
            }
            TypeInner::Sampler { comparison } => {
                record.flag("comparison", *comparison);
            }
        }
        records.push(record.finish());
    }
    tables.insert(("types", -1), records);
}

/// Every struct reachable from a global a host writes (section 8.5).
fn host_structs(module: &Module) -> Vec<bool> {
    let mut marks = vec![false; module.types.len()];
    let mut stack: Vec<_> = module
        .globals
        .iter()
        .filter(|(_, global)| global.space.is_host_shared())
        .map(|(_, global)| global.ty)
        .collect();
    while let Some(ty) = stack.pop() {
        match &module.types[ty].inner {
            TypeInner::Struct { members, .. } if !marks[ty.index()] => {
                marks[ty.index()] = true;
                stack.extend(members.iter().map(|member| member.ty));
            }
            TypeInner::Array { base, .. } => stack.push(*base),
            _ => {}
        }
    }
    marks
}

/// An interface binding's fields, each name after `prefix`.
fn binding(record: &mut Record, binding: Option<&Binding>, prefix: &'static str) {
    let field = |name: &'static str| -> &'static str {
        match (prefix, name) {
            ("", _) => name,
            (_, "builtin") => "result_builtin",
            (_, "location") => "result_location",
            (_, "interpolation") => "result_interpolation",
            (_, "sampling") => "result_sampling",
            _ => "result_blend_src",
        }
    };
    match binding {
        Some(Binding::BuiltIn(builtin)) => {
            record.text(field("builtin"), builtin.name());
        }
        Some(Binding::Location {
            location,
            interpolation,
            sampling,
            blend_src,
        }) => {
            if let Some(location) = location {
                record.int(field("location"), *location);
            }
            if let Some(source) = blend_src {
                record.int(field("blend_src"), *source);
            }
            record
                .optional_text(
                    field("interpolation"),
                    interpolation.map(klf_shader::Interpolation::name),
                )
                .optional_text(field("sampling"), sampling.map(klf_shader::Sampling::name));
        }
        None => {}
    }
}

fn globals(
    module: &Module,
    info: &ModuleInfo,
    resources: &[Option<Resource>],
    target: Target,
    values: &mut Vec<Record>,
) -> Vec<Record> {
    let mut uses = vec![GlobalUse::default(); module.globals.len()];
    for entry in 0..module.entry_points.len() {
        for (total, used) in uses.iter_mut().zip(info.entry_globals(module, entry)) {
            total.read |= used.read;
            total.write |= used.write;
        }
    }
    module
        .globals
        .iter()
        .map(|(handle, global)| {
            let (word, access) = space_word(global.space);
            let mut record = Record::new("global");
            record
                .text("name", &global.name)
                .int("type", global.ty.index() as i64)
                .text("space", word)
                .optional_text("access", access.as_deref())
                .handle("init", global.init)
                .flag("used", uses[handle.index()].any())
                .flag("read", uses[handle.index()].read)
                .flag("written", uses[handle.index()].write);
            if let Some(at) = global
                .init
                .and_then(|init| value(module, info, init, global.ty, values))
            {
                record.int("value", at);
            }
            if let Some(binding) = global.binding {
                record
                    .int("group", binding.group)
                    .int("binding", binding.binding);
            }
            if let Some(resource) = resources.get(handle.index()).and_then(Option::as_ref) {
                let slot = resource.slot(target);
                record
                    .text("resource", resource.kind.name())
                    .int("slot", slot.binding)
                    .int("slot_space", slot.group)
                    .optional_text("slot_name", slot.glsl_name.as_deref())
                    .handle("paired_sampler", resource.paired_sampler);
                if target == Target::Hlsl {
                    record.text("slot_class", hlsl_class(module, global).name());
                }
                if let Some(length) = resource.msl_length {
                    record.int("count_slot", 16 + length);
                }
                if target == Target::Msl {
                    record
                        .int("argument", resource.msl_argument.index)
                        .int("vertex_argument", resource.msl_argument.vertex);
                }
            }
            record.finish()
        })
        .collect()
}

/// The globals `function` reaches through itself and every callee.
fn reached(info: &ModuleInfo, which: FunctionRef) -> Vec<i64> {
    let mut used = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![info.function(which)];
    while let Some(function) = stack.pop() {
        used.extend(
            function
                .globals
                .iter()
                .enumerate()
                .filter(|(_, usage)| usage.any())
                .map(|(index, _)| index as i64),
        );
        for callee in &function.calls {
            if seen.insert(*callee) {
                stack.push(&info.functions[callee.index()]);
            }
        }
    }
    used.into_iter().collect()
}

fn functions(module: &Module, info: &ModuleInfo, tables: &mut Tables) {
    let mut records = Vec::new();
    for which in module.function_refs() {
        let function = module.function(which);
        let at = number(module, which);
        let mut record = Record::new(match which {
            FunctionRef::Function(_) => "function",
            FunctionRef::Entry(_) => "entry",
        });
        let name = match which {
            FunctionRef::Function(_) => &function.name,
            FunctionRef::Entry(index) => &module.entry_points[index].name,
        };
        record
            .text("name", name)
            .int("arguments", function.arguments.len() as i64)
            .int("locals", function.locals.len() as i64)
            .int("expressions", function.expressions.len() as i64)
            .ints(
                "calls",
                info.function(which)
                    .calls
                    .iter()
                    .map(|callee| callee.index() as i64)
                    .collect(),
            )
            .ints("uses", reached(info, which))
            .flag("may_kill", info.function(which).may_kill);
        if let Some(result) = &function.result {
            record.int("result", result.ty.index() as i64);
            binding(&mut record, result.binding.as_ref(), "result");
        }
        if let FunctionRef::Entry(index) = which {
            let entry = &module.entry_points[index];
            record.text("stage", entry.stage.name()).optional_text(
                "early_depth_test",
                entry.early_depth_test.map(klf_shader::EarlyDepthTest::name),
            );
            if let Some(size) = entry.workgroup_size {
                let fixed = |extent: &WorkgroupExtent| match extent {
                    WorkgroupExtent::Fixed(value) => i64::from(*value),
                    WorkgroupExtent::Override(_) => -1,
                };
                let item = |extent: &WorkgroupExtent| match extent {
                    WorkgroupExtent::Fixed(_) => -1,
                    WorkgroupExtent::Override(expression) => expression.index() as i64,
                };
                record
                    .ints("workgroup", size.iter().map(fixed).collect())
                    .ints("workgroup_override", size.iter().map(item).collect());
            }
        }
        records.push(record.finish());
        tables.insert(
            ("arguments", at),
            function
                .arguments
                .iter()
                .map(|argument| {
                    let mut item = Record::new("argument");
                    item.optional_text("name", argument.name.as_deref())
                        .int("type", argument.ty.index() as i64);
                    binding(&mut item, argument.binding.as_ref(), "");
                    item.finish()
                })
                .collect(),
        );
        tables.insert(
            ("locals", at),
            function
                .locals
                .iter()
                .map(|(_, local)| {
                    Record::new("local")
                        .optional_text("name", local.name.as_deref())
                        .int("type", local.ty.index() as i64)
                        .handle("init", local.init)
                        .finish()
                })
                .collect(),
        );
    }
    tables.insert(("functions", -1), records);
}

/// A literal's value without its scalar suffix.
pub(crate) fn literal_value(literal: klf_shader::Literal) -> String {
    let text = literal_text(literal);
    let scalar = literal.scalar().text();
    text.strip_suffix(scalar.as_str())
        .unwrap_or(&text)
        .to_owned()
}
