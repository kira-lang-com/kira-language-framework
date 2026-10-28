//! Binding assignment (`KLF-KSL.md` section 20): the slot every resource of
//! a shader takes on every target, Metal length buffers, and the sampler
//! each texture pairs with.

#![allow(clippy::cast_possible_truncation)]

mod glsl_names;

pub use glsl_names::glsl_safe_name;

use klf_shader::visit::statements;
use klf_shader::{
    AddressSpace, ArraySize, Expression, Function, GlobalVariable, Handle, HlslClass, Module,
    Statement, Target, TargetSlot, Type, TypeInner,
};
use klf_shader_valid::ValidationError;
use std::collections::BTreeSet;

/// The order targets are assigned and reflected in: the oracle's, then the
/// targets it did not have.
pub const TARGETS: [Target; 6] = [
    Target::Msl,
    Target::Wgsl,
    Target::Glsl,
    Target::Hlsl,
    Target::Spirv,
    Target::Ksl,
];

/// What a resource is to a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Uniform,
    Storage,
    Texture,
    Sampler,
}

impl ResourceKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Uniform => "uniform",
            Self::Storage => "storage",
            Self::Texture => "texture",
            Self::Sampler => "sampler",
        }
    }

    /// The kind of a resource global, when it is one.
    #[must_use]
    pub fn of(module: &Module, global: &GlobalVariable) -> Option<Self> {
        match (global.space, &module.types[global.ty].inner) {
            (AddressSpace::Uniform, _) => Some(Self::Uniform),
            (AddressSpace::Storage { .. }, _) => Some(Self::Storage),
            (AddressSpace::Handle, TypeInner::Image { .. }) => Some(Self::Texture),
            (AddressSpace::Handle, TypeInner::Sampler { .. }) => Some(Self::Sampler),
            _ => None,
        }
    }
}

/// Where a resource lands on one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub target: Target,
    pub group: u32,
    pub binding: u32,
    /// The GLSL name a GL host finds it by.
    pub glsl_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub global: Handle<GlobalVariable>,
    pub group: usize,
    pub kind: ResourceKind,
    /// One slot per target, in [`TARGETS`] order.
    pub slots: Vec<Slot>,
    /// The Metal length index `k`; the buffer is at `16 + k`.
    pub msl_length: Option<u32>,
    /// Where emitted MSL declares the resource (section 20.6).
    pub msl_argument: MslArgument,
    pub paired_sampler: Option<Handle<GlobalVariable>>,
}

/// The Metal argument-table index an MSL entry point declares a resource at,
/// which is the one a host binds into (section 20.6). It is not the reflected
/// `msl` slot: the oracle's MSL backend declares every resource at its WGSL
/// binding, and hosts bind there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MslArgument {
    /// In a fragment or compute entry point.
    pub index: u32,
    /// In a vertex entry point, where buffer 0 holds the attribute stream.
    pub vertex: u32,
}

impl MslArgument {
    fn of(global: &GlobalVariable, kind: ResourceKind, wgsl: u32) -> Self {
        let authored = global.slots.iter().find_map(|slot| match slot {
            TargetSlot::MslBuffer(slot)
            | TargetSlot::MslTexture(slot)
            | TargetSlot::MslSampler(slot) => Some(*slot),
            _ => None,
        });
        if let Some(index) = authored {
            return Self {
                index,
                vertex: index,
            };
        }
        let vertex = match kind {
            ResourceKind::Uniform | ResourceKind::Storage => wgsl + 1,
            ResourceKind::Texture | ResourceKind::Sampler => wgsl,
        };
        Self {
            index: wgsl,
            vertex,
        }
    }
}

impl Resource {
    #[must_use]
    pub fn slot(&self, target: Target) -> &Slot {
        self.slots
            .iter()
            .find(|slot| slot.target == target)
            .unwrap_or(&self.slots[0])
    }
}

/// The assignment of one shader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assignment {
    pub resources: Vec<Resource>,
}

impl Assignment {
    #[must_use]
    pub fn resource(&self, global: Handle<GlobalVariable>) -> Option<&Resource> {
        self.resources
            .iter()
            .find(|resource| resource.global == global)
    }

    /// The resources of `original` as the globals of `unit`, a module the
    /// passes derived from it: indexed by the unit's global handles, which
    /// compaction renumbers, and matched by name.
    #[must_use]
    pub fn for_unit(&self, original: &Module, unit: &Module) -> Vec<Option<Resource>> {
        let in_unit = |global: Handle<GlobalVariable>| {
            let name = &original.globals[global].name;
            unit.globals
                .iter()
                .find(|(_, candidate)| &candidate.name == name)
                .map(|(handle, _)| handle)
        };
        let mut found = vec![None; unit.globals.len()];
        for resource in &self.resources {
            if let Some(handle) = in_unit(resource.global) {
                let mut moved = resource.clone();
                moved.global = handle;
                moved.paired_sampler = resource.paired_sampler.and_then(in_unit);
                found[handle.index()] = Some(moved);
            }
        }
        found
    }

    /// The slot of `global` on `target`.
    #[must_use]
    pub fn slot(&self, global: Handle<GlobalVariable>, target: Target) -> Option<&Slot> {
        self.resource(global).map(|resource| resource.slot(target))
    }
}

#[derive(Default)]
struct Counters {
    msl_buffer: u32,
    msl_texture: u32,
    msl_sampler: u32,
    hlsl_buffer: u32,
    hlsl_texture: u32,
    hlsl_sampler: u32,
    glsl_block: u32,
    glsl_texture: u32,
}

fn next(counter: &mut u32) -> u32 {
    let at = *counter;
    *counter += 1;
    at
}

fn authored(global: &GlobalVariable, target: Target) -> Option<u32> {
    global.slots.iter().find_map(|slot| match (slot, target) {
        (TargetSlot::Universal(slot), _)
        | (
            TargetSlot::MslBuffer(slot)
            | TargetSlot::MslTexture(slot)
            | TargetSlot::MslSampler(slot),
            Target::Msl,
        )
        | (TargetSlot::Glsl(slot), Target::Glsl)
        | (TargetSlot::Hlsl { register: slot, .. }, Target::Hlsl) => Some(*slot),
        _ => None,
    })
}

impl Counters {
    fn assign(
        &mut self,
        target: Target,
        global: &GlobalVariable,
        kind: ResourceKind,
        group: u32,
        within: u32,
    ) -> Slot {
        let glsl_name = (target == Target::Glsl).then(|| glsl_safe_name(&global.name));
        if let Some(binding) = authored(global, target) {
            return Slot {
                target,
                group,
                binding,
                glsl_name,
            };
        }
        let (group, binding) = match (target, kind) {
            (Target::Msl, ResourceKind::Uniform | ResourceKind::Storage) => {
                self.msl_buffer += 1;
                (0, self.msl_buffer)
            }
            (Target::Msl, ResourceKind::Texture) => (0, next(&mut self.msl_texture)),
            (Target::Msl, ResourceKind::Sampler) => (0, next(&mut self.msl_sampler)),
            (Target::Wgsl | Target::Spirv | Target::Ksl, _) => (group, within),
            (Target::Hlsl, ResourceKind::Uniform) => (0, next(&mut self.hlsl_buffer)),
            (Target::Hlsl, ResourceKind::Storage | ResourceKind::Texture) => {
                (0, next(&mut self.hlsl_texture))
            }
            (Target::Hlsl, ResourceKind::Sampler) => (0, next(&mut self.hlsl_sampler)),
            (Target::Glsl, ResourceKind::Uniform | ResourceKind::Storage) => {
                (0, next(&mut self.glsl_block))
            }
            (Target::Glsl, ResourceKind::Texture) => (0, next(&mut self.glsl_texture)),
            (Target::Glsl, ResourceKind::Sampler) => (0, 0),
        };
        Slot {
            target,
            group,
            binding,
            glsl_name,
        }
    }
}

/// Whether `ty` is or ends in a runtime-sized array.
#[must_use]
pub fn runtime_sized(module: &Module, ty: Handle<Type>) -> bool {
    match &module.types[ty].inner {
        TypeInner::Array {
            size: ArraySize::Runtime,
            ..
        } => true,
        TypeInner::Struct { members, .. } => members
            .last()
            .is_some_and(|member| runtime_sized(module, member.ty)),
        _ => false,
    }
}

/// The sampler the first sample naming `texture` uses in `function`.
fn sampler_in(
    function: &Function,
    texture: Handle<GlobalVariable>,
) -> Option<Handle<GlobalVariable>> {
    let global = |handle: Handle<Expression>| match function.expressions.get(handle) {
        Some(Expression::GlobalVariable(global)) => Some(*global),
        _ => None,
    };
    function
        .expressions
        .iter()
        .find_map(|(_, expression)| match expression {
            Expression::ImageSample { image, sampler, .. } if global(*image) == Some(texture) => {
                global(*sampler)
            }
            _ => None,
        })
}

/// The functions an entry reaches, in arena order.
fn helpers(module: &Module, entry: &Function) -> Vec<Handle<Function>> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![entry];
    while let Some(function) = stack.pop() {
        statements(&function.body, &mut |statement| {
            if let Statement::Call {
                function: callee, ..
            } = statement
                && seen.insert(*callee)
            {
                stack.push(&module.functions[*callee]);
            }
        });
    }
    seen.into_iter().collect()
}

/// Assigns the slots of shader `shader` (section 20).
///
/// # Errors
/// Returns `shader.slot-collision` for two resources on one slot.
pub fn assign(module: &Module, shader: usize) -> Result<Assignment, Vec<ValidationError>> {
    let declaration = &module.shaders[shader];
    let mut counters = Counters::default();
    let mut resources = Vec::new();
    for (group_index, group) in declaration.groups.iter().enumerate() {
        for (position, handle) in group.globals.iter().enumerate() {
            let global = &module.globals[*handle];
            let Some(kind) = ResourceKind::of(module, global) else {
                continue;
            };
            let within = global
                .binding
                .map_or(position as u32, |binding| binding.binding);
            let slots: Vec<Slot> = TARGETS
                .iter()
                .map(|target| counters.assign(*target, global, kind, group_index as u32, within))
                .collect();
            let wgsl = slots
                .iter()
                .find(|slot| slot.target == Target::Wgsl)
                .map_or(within, |slot| slot.binding);
            let paired_sampler = match kind {
                ResourceKind::Texture => paired_sampler(module, shader, *handle)
                    .or_else(|| declared_sampler(module, &group.globals, position)),
                _ => None,
            };
            resources.push(Resource {
                global: *handle,
                group: group_index,
                kind,
                slots,
                msl_length: None,
                msl_argument: MslArgument::of(global, kind, wgsl),
                paired_sampler,
            });
        }
    }
    for resource in &mut resources {
        if resource.kind == ResourceKind::Storage
            && runtime_sized(module, module.globals[resource.global].ty)
        {
            counters.msl_buffer += 1;
            resource.msl_length = Some(counters.msl_buffer);
        }
    }
    let assignment = Assignment { resources };
    let problems = collisions(module, &assignment);
    if problems.is_empty() {
        Ok(assignment)
    } else {
        Err(problems)
    }
}

fn paired_sampler(
    module: &Module,
    shader: usize,
    texture: Handle<GlobalVariable>,
) -> Option<Handle<GlobalVariable>> {
    module.shaders[shader].entries.iter().find_map(|entry| {
        let function = &module.entry_points[*entry].function;
        sampler_in(function, texture).or_else(|| {
            helpers(module, function)
                .into_iter()
                .find_map(|helper| sampler_in(&module.functions[helper], texture))
        })
    })
}

fn declared_sampler(
    module: &Module,
    globals: &[Handle<GlobalVariable>],
    position: usize,
) -> Option<Handle<GlobalVariable>> {
    let is_sampler = |handle: &&Handle<GlobalVariable>| {
        ResourceKind::of(module, &module.globals[**handle]) == Some(ResourceKind::Sampler)
    };
    globals
        .iter()
        .skip(position + 1)
        .find(is_sampler)
        .or_else(|| globals.iter().find(is_sampler))
        .copied()
}

/// The register class an HLSL resource is declared on.
#[must_use]
pub fn hlsl_class(module: &Module, global: &GlobalVariable) -> HlslClass {
    match (global.space, &module.types[global.ty].inner) {
        (AddressSpace::Uniform, _) => HlslClass::B,
        (AddressSpace::Storage { access }, _) if access.store || access.atomic => HlslClass::U,
        (_, TypeInner::Sampler { .. }) => HlslClass::S,
        (
            _,
            TypeInner::Image {
                class: klf_shader::ImageClass::Storage { .. },
                ..
            },
        ) => HlslClass::U,
        _ => HlslClass::T,
    }
}

/// A target slot: group, binding and the space it is numbered in.
type SlotKey<'a> = (u32, u32, &'a str);

fn collisions(module: &Module, assignment: &Assignment) -> Vec<ValidationError> {
    let mut problems = Vec::new();
    for target in TARGETS {
        let mut taken: Vec<(SlotKey<'_>, Handle<GlobalVariable>)> = Vec::new();
        for resource in &assignment.resources {
            let slot = resource.slot(target);
            let global = &module.globals[resource.global];
            let space = match (target, resource.kind) {
                (Target::Msl, ResourceKind::Uniform | ResourceKind::Storage) => "buffer",
                (Target::Msl, ResourceKind::Texture) => "texture",
                (Target::Msl, ResourceKind::Sampler) => "sampler",
                (Target::Hlsl, _) => hlsl_class(module, global).name(),
                (Target::Glsl, ResourceKind::Uniform | ResourceKind::Storage) => "block",
                (Target::Glsl, ResourceKind::Texture) => "unit",
                (Target::Glsl, ResourceKind::Sampler) => continue,
                (Target::Wgsl | Target::Spirv | Target::Ksl, _) => "binding",
            };
            let key = (slot.group, slot.binding, space);
            if let Some((_, other)) = taken.iter().find(|(used, _)| *used == key) {
                problems.push(ValidationError::keyed(
                    "SV13",
                    "shader.slot-collision",
                    format!(
                        "`{}` and `{}` land on one {} slot",
                        module.globals[*other].name,
                        global.name,
                        target.name()
                    ),
                    module.globals.span(resource.global),
                ));
            }
            taken.push((key, resource.global));
        }
    }
    problems
}
