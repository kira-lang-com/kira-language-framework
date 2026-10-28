//! What a shader module shows in an editor (`KLF-EDITOR.md` sections 14.3
//! and 14.4).
//!
//! Everything here is computed from the module, its binding assignment and
//! what the definition wrote in it. Nothing asks the definition for anything:
//! a shader definition records its own tokens and symbols, and the kernel
//! adds the binding, layout and option facts over them.

mod builtins;

use klf_core::FileId;
use klf_facts::{
    Entity, Facts, Hint, HintKind, Runnable, RunnableKind, Symbol, SymbolKind, TokenClass,
    TokenModifier,
};
use klf_shader::layout::host_shareable;
use klf_shader::print;
use klf_shader::{
    AddressSpace, BuiltIn, GlobalVariable, Handle, Module, Override, Span, Target, Type, TypeInner,
};
use klf_shader_bind::{Assignment, ResourceKind, assign, hlsl_class};
use std::fmt::Write;

/// Kernel entities are numbered from `1 << 63`, so one cannot collide with an
/// identifier a definition allocated for itself.
const KERNEL: u64 = 1 << 63;

/// What the language's own declaration says about its outputs.
#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    /// The targets the language builds for, in the order it lists them.
    pub targets: &'a [Target],
    /// Whether the options stay overrides on a target that keeps them.
    pub keep_overrides: bool,
}

/// Records what `module` shows in an editor. `file_of` names the file a span
/// points into; a span in a file the analysis does not hold is left out.
#[must_use]
pub fn record(
    module: &Module,
    settings: &Settings<'_>,
    file_of: &dyn Fn(&str) -> Option<FileId>,
) -> Facts {
    let mut facts = Facts::default();
    let mut recorder = Recorder {
        module,
        settings,
        file_of,
        facts: &mut facts,
        entity: 0,
        structs: Vec::new(),
    };
    for index in 0..module.shaders.len() {
        if let Ok(assignment) = assign(module, index) {
            recorder.resources(index, &assignment);
            recorder.options(&module.shaders[index].options);
        }
    }
    recorder.entries();
    recorder.samples();
    recorder.shaders();
    recorder.structs();
    facts
}

struct Recorder<'a> {
    module: &'a Module,
    settings: &'a Settings<'a>,
    file_of: &'a dyn Fn(&str) -> Option<FileId>,
    facts: &'a mut Facts,
    entity: u64,
    /// The host-shareable structs already recorded, by handle.
    structs: Vec<Handle<Type>>,
}

impl Recorder<'_> {
    fn at(&self, span: Span) -> Option<(FileId, u32, u32)> {
        if !span.is_known() {
            return None;
        }
        let file = (self.file_of)(self.module.files.get(span.file as usize)?)?;
        Some((file, span.start, span.end))
    }

    fn next_entity(&mut self) -> Entity {
        self.entity += 1;
        Entity(KERNEL | self.entity)
    }

    /// A symbol the kernel records over a span the definition wrote, and the
    /// token class it refines that name to.
    fn symbol(
        &mut self,
        span: Span,
        name: &str,
        kind: SymbolKind,
        class: TokenClass,
        modifiers: u32,
        documentation: String,
    ) {
        let Some((file, start, end)) = self.at(span) else {
            return;
        };
        let entity = self.next_entity();
        self.facts.declare_symbol(
            entity,
            Symbol {
                name: name.to_owned(),
                kind: Some(kind),
                file: Some(file),
                start,
                end,
                selection_start: start,
                selection_end: end,
                container: None,
                signature: String::new(),
                documentation,
                detail: String::new(),
                parameters: Vec::new(),
                deprecated: false,
                exported: true,
                local: false,
            },
        );
        self.facts
            .declaration(entity, file, (start, end), (start, end));
        self.facts.classify(file, start, end, class, modifiers);
    }

    fn hint(&mut self, span: Span, label: String, padding_left: bool) {
        let Some((file, at, _)) = self.at(span) else {
            return;
        };
        self.facts.hint(
            file,
            Hint {
                at,
                label,
                kind: HintKind::Layout,
                padding_left,
                padding_right: true,
                target: None,
                insert: None,
            },
        );
    }

    /// A resource: the neutral binding, what each target calls it, its paired
    /// sampler, its Metal length slot and any slot the source authored
    /// (section 14.3).
    fn resources(&mut self, shader: usize, assignment: &Assignment) {
        for resource in &assignment.resources {
            let global = resource.global;
            let span = self.module.globals.span(global);
            let declared = self.module.shaders[shader].groups.get(resource.group);
            let group =
                declared.map_or_else(|| resource.group.to_string(), |group| group.name.clone());
            let class = declared.map_or_else(
                || "custom".to_owned(),
                |group| format!("{:?}", group.class).to_lowercase(),
            );
            let neutral = self.module.globals[global].binding.map_or_else(
                || resource.slot(Target::Spirv).binding,
                |binding| binding.binding,
            );
            let mut text = format!(
                "`{}` in group `{group}` of class `{class}`\n\nNeutral binding: `({}, {neutral})`",
                resource.kind.name(),
                resource.group,
            );
            if let Some(sampler) = resource.paired_sampler {
                let _ = write!(
                    text,
                    "\n\nPaired sampler: `{}`",
                    self.module.globals[sampler].name
                );
            }
            if let Some(length) = resource.msl_length {
                let _ = write!(
                    text,
                    "\n\nMetal length: `{length}`, its buffer at `{}`",
                    16 + length
                );
            }
            let authored = self.authored_slots(global);
            if !authored.is_empty() {
                let _ = write!(text, "\n\nAuthored: {}", authored.join(", "));
            }
            text.push_str("\n\n| Target | Slot |\n| --- | --- |\n");
            for target in self.settings.targets {
                let slot = resource.slot(*target);
                let _ = writeln!(
                    text,
                    "| `{}` | {} |",
                    target.name(),
                    self.slot_text(
                        *target,
                        global,
                        resource.kind,
                        slot.group,
                        slot.binding,
                        slot.glsl_name.as_deref()
                    )
                );
            }
            let modifiers = TokenModifier::Resource.bit()
                | if self.writable(global) {
                    TokenModifier::Mutable.bit()
                } else {
                    0
                };
            let name = self.module.globals[global].name.clone();
            self.symbol(
                span,
                &name,
                SymbolKind::Resource,
                TokenClass::Variable,
                modifiers,
                text,
            );
        }
    }

    fn writable(&self, global: Handle<GlobalVariable>) -> bool {
        match self.module.globals[global].space {
            AddressSpace::Storage { access } => access.store || access.atomic,
            _ => false,
        }
    }

    fn authored_slots(&self, global: Handle<GlobalVariable>) -> Vec<String> {
        self.module.globals[global]
            .slots
            .iter()
            .map(|slot| match *slot {
                klf_shader::TargetSlot::Universal(value) => format!("universal `{value}`"),
                klf_shader::TargetSlot::MslBuffer(value) => format!("Metal buffer `{value}`"),
                klf_shader::TargetSlot::MslTexture(value) => format!("Metal texture `{value}`"),
                klf_shader::TargetSlot::MslSampler(value) => format!("Metal sampler `{value}`"),
                klf_shader::TargetSlot::Hlsl {
                    class,
                    register,
                    space,
                } => format!("HLSL `{}`{register} in space `{space}`", class_name(class)),
                klf_shader::TargetSlot::Glsl(value) => format!("GLSL `binding = {value}`"),
            })
            .collect()
    }

    fn slot_text(
        &self,
        target: Target,
        global: Handle<GlobalVariable>,
        kind: ResourceKind,
        group: u32,
        binding: u32,
        glsl_name: Option<&str>,
    ) -> String {
        match target {
            Target::Msl => format!(
                "`[[{}({binding})]]`",
                match kind {
                    ResourceKind::Uniform | ResourceKind::Storage => "buffer",
                    ResourceKind::Texture => "texture",
                    ResourceKind::Sampler => "sampler",
                }
            ),
            Target::Hlsl => {
                let class = hlsl_class(self.module, &self.module.globals[global]);
                format!("`{}{binding}`", class_name(class))
            }
            Target::Glsl => match glsl_name {
                Some(name) => format!("`binding = {binding}`, `{name}`"),
                None => format!("`binding = {binding}`"),
            },
            Target::Wgsl => format!("`@group({group}) @binding({binding})`"),
            Target::Spirv | Target::Ksl => format!("group `{group}`, binding `{binding}`"),
        }
    }

    /// An option: its type, default and id, and whether the targets keep or
    /// bake it (section 14.3).
    fn options(&mut self, options: &[Handle<Override>]) {
        for handle in options {
            let span = self.module.overrides.span(*handle);
            let option = &self.module.overrides[*handle];
            let default = option.init.map_or_else(
                || "none".to_owned(),
                |init| print::expression_text(self.module, init),
            );
            let id = option
                .id
                .map_or_else(|| "its position".to_owned(), |id| id.to_string());
            let fate = if self.settings.keep_overrides {
                "kept as an override"
            } else {
                "baked into the output"
            };
            let text = format!(
                "Type: `{}`\n\nDefault: `{default}`\n\nId: {id}\n\nTargets: {fate}",
                print::type_text(self.module, option.ty)
            );
            let name = option.name.clone();
            self.symbol(
                span,
                &name,
                SymbolKind::Constant,
                TokenClass::Constant,
                0,
                text,
            );
        }
    }

    /// A stage interface member: what it is bound to and how it interpolates
    /// (section 14.3).
    fn entries(&mut self) {
        // Collected first: recording needs `&mut self`, and the module is
        // behind a shared borrow of it.
        let mut members = Vec::new();
        for entry in &self.module.entry_points {
            for argument in &entry.function.arguments {
                let Some(binding) = argument.binding else {
                    continue;
                };
                let name = argument
                    .name
                    .clone()
                    .unwrap_or_else(|| binding_name(binding).to_owned());
                let text = match binding {
                    klf_shader::Binding::BuiltIn(builtin) => builtin_text(builtin),
                    klf_shader::Binding::Location {
                        location,
                        interpolation,
                        sampling,
                        blend_src,
                    } => {
                        let mut text = format!(
                            "Location: `{}`",
                            location.map_or_else(|| "auto".to_owned(), |value| value.to_string())
                        );
                        if let Some(interpolation) = interpolation {
                            let _ = write!(text, "\n\nInterpolation: `{interpolation:?}`");
                        }
                        if let Some(sampling) = sampling {
                            let _ = write!(text, "\n\nSampling: `{sampling:?}`");
                        }
                        if let Some(blend_src) = blend_src {
                            let _ = write!(text, "\n\nBlend source: `{blend_src}`");
                        }
                        text
                    }
                };
                members.push((entry.span, name, text));
            }
        }
        for (span, name, text) in members {
            self.symbol(
                span,
                &name,
                SymbolKind::Field,
                TokenClass::Property,
                TokenModifier::StageIo.bit(),
                text,
            );
        }
    }

    /// A sample call: the level it lowers to, which is what a target can
    /// only spell in some cases (`KLF-KSL.md` section 22.4, DS-009).
    fn samples(&mut self) {
        let mut samples = Vec::new();
        for (_, function) in self.module.functions.iter() {
            for (handle, expression) in function.expressions.iter() {
                if let klf_shader::Expression::ImageSample { level, .. } = expression {
                    samples.push((function.expressions.span(handle), *level));
                }
            }
        }
        for (span, level) in samples {
            let name = match level {
                klf_shader::SampleLevel::Auto => "automatic".to_owned(),
                klf_shader::SampleLevel::Zero => "level zero".to_owned(),
                klf_shader::SampleLevel::Exact(_) => "an explicit level".to_owned(),
                klf_shader::SampleLevel::Bias(_) => "level plus a bias".to_owned(),
                klf_shader::SampleLevel::Gradient { .. } => "the gradient".to_owned(),
            };
            self.symbol(
                span,
                "sample",
                SymbolKind::Function,
                TokenClass::Function,
                0,
                format!("Sample level: {name}"),
            );
        }
    }

    /// A shader declaration: a runnable of kind `Shader` (section 21.2 item
    /// 4), which is also the lens the output views hang off.
    fn shaders(&mut self) {
        let targets: Vec<String> = self
            .settings
            .targets
            .iter()
            .map(|target| target.name().to_owned())
            .collect();
        let mut runnables = Vec::new();
        for shader in &self.module.shaders {
            if let Some((file, start, end)) = self.at(shader.span) {
                runnables.push((file, start, end, shader.name.clone()));
            }
        }
        for (file, start, end, name) in runnables {
            self.facts.runnable(
                file,
                Runnable {
                    label: name.clone(),
                    kind: RunnableKind::Shader,
                    start,
                    end,
                    arguments: vec![name, targets.join(",")],
                },
            );
        }
    }

    /// Every host-shareable struct a uniform or storage resource reaches: its
    /// layout, and a hint per member and per gap (section 14.4).
    fn structs(&mut self) {
        // The types are collected before any is recorded, for the same
        // reason as in `entries`.
        let mut roots = Vec::new();
        for (_, global) in self.module.globals.iter() {
            if matches!(
                global.space,
                AddressSpace::Uniform | AddressSpace::Storage { .. }
            ) {
                roots.push(global.ty);
            }
        }
        let mut pending = roots;
        while let Some(ty) = pending.pop() {
            if !host_shareable(&self.module.types, ty) || self.structs.contains(&ty) {
                continue;
            }
            self.structs.push(ty);
            let TypeInner::Struct { members, .. } = &self.module.types[ty].inner else {
                continue;
            };
            let members = members.clone();
            pending.extend(members.iter().map(|member| member.ty));
            self.struct_layout(ty, &members);
        }
    }

    fn struct_layout(&mut self, ty: Handle<Type>, members: &[klf_shader::StructMember]) {
        let TypeInner::Struct {
            rule, size, align, ..
        } = self.module.types[ty].inner
        else {
            return;
        };
        let (rule, size, align) = (rule, size, align);
        let mut text = format!(
            "Layout rule: `{rule:?}` · size `{size}` · alignment `{align}`\n\n\
             | Member | Offset | Size | Alignment |\n| --- | --- | --- | --- |\n"
        );
        for member in members {
            let _ = writeln!(
                text,
                "| `{}` | `{}` | `{}` | `{}` |",
                member.name, member.offset, member.size, member.align
            );
        }
        let name = self.module.types[ty]
            .name
            .clone()
            .unwrap_or_else(|| print::type_text(self.module, ty));
        let span = self.module.types.span(ty);
        self.symbol(span, &name, SymbolKind::Struct, TokenClass::Struct, 0, text);
        for (index, member) in members.iter().enumerate() {
            if !member.span.is_known() {
                continue;
            }
            let end = members[..index]
                .iter()
                .map(|known| known.offset + known.size)
                .max()
                .unwrap_or(0);
            if member.offset > end {
                self.hint(
                    member.span,
                    format!("{} bytes padding", member.offset - end),
                    false,
                );
            }
            self.hint(
                member.span,
                format!("offset {} · size {}", member.offset, member.size),
                true,
            );
        }
    }
}

fn class_name(class: klf_shader::HlslClass) -> &'static str {
    match class {
        klf_shader::HlslClass::B => "b",
        klf_shader::HlslClass::T => "t",
        klf_shader::HlslClass::U => "u",
        klf_shader::HlslClass::S => "s",
    }
}

fn binding_name(binding: klf_shader::Binding) -> &'static str {
    match binding {
        klf_shader::Binding::BuiltIn(builtin) => builtin_name(builtin),
        klf_shader::Binding::Location { .. } => "value",
    }
}

/// A built-in: its name in KSL and its spelling on every target
/// (`KLF-EDITOR.md` section 14.3).
fn builtin_text(builtin: BuiltIn) -> String {
    let spellings = builtins::spellings(builtin);
    let mut text = format!(
        "Built-in `{}`\n\n| Target | Spelling |\n| --- | --- |\n",
        builtin_name(builtin)
    );
    for target in Target::ALL {
        let spelling = match spellings.get(target) {
            Some(Some(spelling)) => format!("`{spelling}`"),
            _ => "not available".to_owned(),
        };
        let _ = writeln!(text, "| `{}` | {spelling} |", target.name());
    }
    text
}

fn builtin_name(builtin: BuiltIn) -> &'static str {
    match builtin {
        BuiltIn::Position => "position",
        BuiltIn::VertexIndex => "vertex_index",
        BuiltIn::InstanceIndex => "instance_index",
        BuiltIn::FrontFacing => "front_facing",
        BuiltIn::FragDepth => "frag_depth",
        BuiltIn::SampleIndex => "sample_index",
        BuiltIn::SampleMask => "sample_mask",
        BuiltIn::GlobalInvocationId => "global_invocation_id",
        BuiltIn::LocalInvocationId => "local_invocation_id",
        BuiltIn::LocalInvocationIndex => "local_invocation_index",
        BuiltIn::WorkgroupId => "workgroup_id",
        BuiltIn::NumWorkgroups => "num_workgroups",
        BuiltIn::ClipDistances => "clip_distances",
    }
}
