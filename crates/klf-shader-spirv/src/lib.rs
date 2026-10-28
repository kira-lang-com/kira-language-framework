//! The SPIR-V backend (`KLF-KSL.md` section 19.4): one module of binary
//! words per entry point, with module-scope interface variables, explicit
//! layout decorations on buffer types, and structured control flow.
//!
//! `too_many_lines` and `match_same_arms` are allowed: the writers are
//! matches over every expression, statement or type kind, which read best
//! whole and with one arm per kind. The casts narrow counts and indices
//! the validator already bounded.
#![allow(
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::cast_possible_truncation
)]

pub mod disasm;
mod entry;
mod expr;
mod function;
mod image;
mod math;
mod spec;
mod types;
mod words;

use klf_shader::{AddressSpace, Handle, Module, Span, Stage, Target, TypeInner};
use klf_shader_bind::Resource;
use klf_shader_valid::{ModuleInfo, ValidationError};
use std::collections::{BTreeSet, HashMap};
use types::{ConstKey, TypeKey};
use words::{capability, class, decoration, instruction, op, string};

/// The SPIR-V version a module declares, as the header word encodes it.
const VERSION: u32 = 0x0001_0300;
const MAGIC: u32 = 0x0723_0203;

/// How a module-scope variable is reached.
#[derive(Clone, Copy)]
struct GlobalId {
    variable: u32,
    /// Whether the variable is a block wrapping the global's type as
    /// member 0.
    wrapped: bool,
}

pub(crate) struct Writer<'a> {
    module: &'a Module,
    info: &'a ModuleInfo,
    next: u32,
    capabilities: BTreeSet<u32>,
    glsl: u32,
    /// Whether `OpSource` and `OpLine` place the code in its source.
    debug_info: bool,
    /// The `OpString` of each source file named so far, by file index.
    files: HashMap<u32, u32>,
    sources: Vec<u32>,
    debug: Vec<u32>,
    annotations: Vec<u32>,
    declarations: Vec<u32>,
    functions: Vec<u32>,
    types: HashMap<TypeKey, u32>,
    constants: HashMap<ConstKey, u32>,
    pointers: HashMap<(u32, u32), u32>,
    function_types: HashMap<Vec<u32>, u32>,
    blocks: BTreeSet<u32>,
    globals: Vec<Option<GlobalId>>,
    function_ids: Vec<u32>,
    overrides: HashMap<usize, u32>,
    interface: Vec<u32>,
    problems: Vec<(String, Span)>,
}

/// Writes the SPIR-V module of a unit that holds exactly one entry point.
///
/// # Errors
/// Returns `shader.target-unsupported` for each construct SPIR-V cannot
/// express.
pub fn write(
    module: &Module,
    info: &ModuleInfo,
    resources: &[Option<Resource>],
    debug_info: bool,
) -> Result<Vec<u32>, Vec<ValidationError>> {
    let mut writer = Writer {
        module,
        info,
        next: 1,
        capabilities: BTreeSet::from([capability::SHADER]),
        glsl: 0,
        debug_info,
        files: HashMap::new(),
        sources: Vec::new(),
        debug: Vec::new(),
        annotations: Vec::new(),
        declarations: Vec::new(),
        functions: Vec::new(),
        types: HashMap::new(),
        constants: HashMap::new(),
        pointers: HashMap::new(),
        function_types: HashMap::new(),
        blocks: BTreeSet::new(),
        globals: Vec::new(),
        function_ids: Vec::new(),
        overrides: HashMap::new(),
        interface: Vec::new(),
        problems: Vec::new(),
    };
    writer.glsl = writer.id();
    let words = writer.module_words(resources);
    if writer.problems.is_empty() {
        Ok(words)
    } else {
        Err(refused(writer.problems))
    }
}

/// The words as lowercase hexadecimal, eight characters per word, the
/// form of the `.spv` files (section 23).
#[must_use]
pub fn hex(words: &[u32]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(words.len() * 8);
    for word in words {
        let _ = write!(text, "{word:08x}");
    }
    text
}

/// The words [`hex`] wrote, read back. The text form is what section 23
/// calls the `.spv` form, and the editor's SPIR-V view starts from it.
#[must_use]
pub fn from_hex(text: &str) -> Vec<u32> {
    let text = text.trim();
    text.as_bytes()
        .chunks(8)
        .filter_map(|chunk| {
            let word = std::str::from_utf8(chunk).ok()?;
            u32::from_str_radix(word, 16).ok()
        })
        .collect()
}

impl Writer<'_> {
    pub(crate) fn id(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }

    pub(crate) fn refuse(&mut self, message: impl Into<String>, span: Span) {
        self.problems.push((message.into(), span));
    }

    pub(crate) fn name(&mut self, id: u32, name: &str) {
        let mut operands = vec![id];
        operands.extend(string(name));
        self.debug.extend(instruction(op::NAME, &operands));
    }

    /// The `OpString` naming source file `file`, declared with its
    /// `OpSource` on first use.
    pub(crate) fn file_string(&mut self, file: u32) -> u32 {
        if let Some(id) = self.files.get(&file) {
            return *id;
        }
        let module = self.module;
        let path = module.files.get(file as usize).map_or("", String::as_str);
        let id = self.id();
        let mut operands = vec![id];
        operands.extend(string(path));
        self.sources.extend(instruction(op::STRING, &operands));
        self.sources.extend(instruction(op::SOURCE, &[0, 0, id]));
        self.files.insert(file, id);
        id
    }

    pub(crate) fn decorate(&mut self, id: u32, decoration: u32, operands: &[u32]) {
        let mut all = vec![id, decoration];
        all.extend_from_slice(operands);
        self.annotations.extend(instruction(op::DECORATE, &all));
    }

    pub(crate) fn declare(&mut self, opcode: u16, operands: &[u32]) {
        self.declarations.extend(instruction(opcode, operands));
    }

    fn module_words(&mut self, resources: &[Option<Resource>]) -> Vec<u32> {
        let module = self.module;
        let Some(entry) = module.entry_points.first() else {
            self.refuse("a SPIR-V unit has one entry point", Span::default());
            return Vec::new();
        };
        if module.entry_points.len() > 1 {
            self.refuse("a SPIR-V unit has one entry point", entry.span);
        }
        for (handle, _) in module.globals.iter() {
            let id = self.global(handle, resources);
            self.globals.push(id);
        }
        for _ in module.functions.iter() {
            let id = self.id();
            self.function_ids.push(id);
        }
        for (handle, function) in module.functions.iter() {
            let id = self.function_ids[handle.index()];
            self.name(id, &function.name);
            function::write_function(
                self,
                id,
                function,
                &self.info.functions[handle.index()],
                None,
            );
        }
        let entry_id = self.id();
        self.name(entry_id, &entry.name);
        function::write_function(
            self,
            entry_id,
            &entry.function,
            &self.info.entries[0],
            Some(entry),
        );
        self.assemble(entry_id)
    }

    fn global(
        &mut self,
        handle: Handle<klf_shader::GlobalVariable>,
        resources: &[Option<Resource>],
    ) -> Option<GlobalId> {
        let global = &self.module.globals[handle];
        let used = self.info.entries[0]
            .globals
            .get(handle.index())
            .is_some_and(|use_| use_.any())
            || self.info.functions.iter().any(|info| {
                info.globals
                    .get(handle.index())
                    .is_some_and(|use_| use_.any())
            });
        if !used {
            return None;
        }
        let storage = storage_class(global.space);
        let ty = &self.module.types[global.ty];
        let wrapped = matches!(
            global.space,
            AddressSpace::Uniform | AddressSpace::Storage { .. } | AddressSpace::Immediate
        ) && !matches!(ty.inner, TypeInner::Struct { .. });
        let data_type = if wrapped {
            self.wrapper(global.ty, global.space)
        } else {
            let id = self.type_of(global.ty, global.space.is_host_shared());
            if global.space.is_host_shared() && self.blocks.insert(id) {
                self.decorate(id, decoration::BLOCK, &[]);
            }
            id
        };
        let pointer = self.pointer(storage, data_type);
        let variable = self.id();
        let mut operands = vec![pointer, variable, storage];
        if let Some(init) = global.init {
            let value = self.global_constant(init, global.ty);
            operands.push(value);
        } else if global.space == AddressSpace::Private {
            let plain = self.type_of(global.ty, false);
            operands.push(self.null(plain));
        }
        self.declare(op::VARIABLE, &operands);
        self.name(variable, &global.name);
        if global.space.is_resource() {
            let binding = resources
                .get(handle.index())
                .and_then(Option::as_ref)
                .map(|resource| {
                    let slot = resource.slot(Target::Spirv);
                    (slot.group, slot.binding)
                })
                .or(global
                    .binding
                    .map(|binding| (binding.group, binding.binding)));
            match binding {
                Some((group, binding)) => {
                    self.decorate(variable, decoration::DESCRIPTOR_SET, &[group]);
                    self.decorate(variable, decoration::BINDING, &[binding]);
                }
                None => self.refuse(
                    format!("resource `{}` has no binding", global.name),
                    self.module.globals.span(handle),
                ),
            }
        }
        match (global.space, &ty.inner) {
            (AddressSpace::Storage { access }, _) if !access.store => {
                self.decorate(variable, decoration::NON_WRITABLE, &[]);
            }
            (
                AddressSpace::Handle,
                TypeInner::Image {
                    class: klf_shader::ImageClass::Storage { access, .. },
                    ..
                },
            ) => {
                if !access.store {
                    self.decorate(variable, decoration::NON_WRITABLE, &[]);
                }
                if !access.load {
                    self.decorate(variable, decoration::NON_READABLE, &[]);
                }
            }
            _ => {}
        }
        Some(GlobalId { variable, wrapped })
    }

    fn assemble(&mut self, entry_id: u32) -> Vec<u32> {
        let entry = &self.module.entry_points[0];
        let mut words = vec![MAGIC, VERSION, 0, self.next, 0];
        for capability in &self.capabilities {
            words.extend(instruction(op::CAPABILITY, &[*capability]));
        }
        let mut import = vec![self.glsl];
        import.extend(string("GLSL.std.450"));
        words.extend(instruction(op::EXT_INST_IMPORT, &import));
        words.extend(instruction(op::MEMORY_MODEL, &[0, 1]));
        let model = match entry.stage {
            Stage::Vertex => 0,
            Stage::Fragment => 4,
            Stage::Compute => 5,
        };
        let mut operands = vec![model, entry_id];
        operands.extend(string(&entry.name));
        operands.extend(&self.interface);
        words.extend(instruction(op::ENTRY_POINT, &operands));
        words.extend(entry::execution_modes(self, entry_id));
        words.extend(&self.sources);
        words.extend(&self.debug);
        words.extend(&self.annotations);
        words.extend(&self.declarations);
        words.extend(&self.functions);
        words[3] = self.next;
        words
    }
}

/// The storage class of an address space.
pub(crate) const fn storage_class(space: AddressSpace) -> u32 {
    match space {
        AddressSpace::Function => class::FUNCTION,
        AddressSpace::Private => class::PRIVATE,
        AddressSpace::Workgroup => class::WORKGROUP,
        AddressSpace::Uniform => class::UNIFORM,
        AddressSpace::Storage { .. } => class::STORAGE_BUFFER,
        AddressSpace::Handle => class::UNIFORM_CONSTANT,
        AddressSpace::Immediate => class::PUSH_CONSTANT,
    }
}

/// The problems of a module SPIR-V cannot express, as the refusals the
/// validator reports.
fn refused(problems: Vec<(String, Span)>) -> Vec<ValidationError> {
    problems
        .into_iter()
        .map(|(message, span)| {
            ValidationError::keyed(
                "B3",
                "shader.target-unsupported",
                format!("{}: {message}", Target::Spirv.name()),
                span,
            )
        })
        .collect()
}
