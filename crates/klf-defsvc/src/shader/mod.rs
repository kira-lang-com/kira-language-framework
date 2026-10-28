//! The shader builder services, `klf.shader.*` (`KLF-KSL.md` section 6): a
//! shader definition's `lower` builds one KSIR module through these.
//!
//! Every service takes machine words and strings; a string crosses as a
//! pointer and a byte length. Handles are arena indices, and `-1` stands
//! for an absent optional. Types, bindings, address spaces and slots are
//! written as textual KSIR spells them (section 18). Spans are not passed
//! per call: `klf.shader.span` sets the one the following nodes carry.
//!
//! A misuse is a definition bug (section 6.4). The first one is kept, every
//! later call answers `-1`, and finishing the module fails the phase with
//! `KLF-SHD-9001`.

mod expr;
pub(crate) mod read;
mod scope;
mod stmt;
mod types;

use crate::{
    DefinitionArtifactKind, DefinitionMaterialization, DefinitionServiceState, ServiceSink,
};
use klf_rt::ServiceMemory;
use klf_services::ShaderSettings;
use klf_shader::builder::{FunctionBuilder, ModuleBuilder};
use klf_shader::{FunctionArgument, GroupDecl, Handle, Override, Span, Type, WorkgroupExtent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const NONE: u64 = u64::MAX;

/// The decoded arguments of one call: its words and its strings, each in
/// order.
pub(super) struct Args {
    words: Vec<u64>,
    texts: Vec<String>,
}

impl Args {
    fn word(&self, index: usize) -> Result<u64, String> {
        self.words
            .get(index)
            .copied()
            .ok_or_else(|| format!("argument {index} is missing"))
    }

    fn int(&self, index: usize) -> Result<i64, String> {
        self.word(index).map(u64::cast_signed)
    }

    fn index(&self, index: usize) -> Result<usize, String> {
        usize::try_from(self.word(index)?).map_err(|_| format!("argument {index} is not a handle"))
    }

    fn handle<T>(&self, index: usize) -> Result<Handle<T>, String> {
        u32::try_from(self.word(index)?)
            .map(Handle::new)
            .map_err(|_| format!("argument {index} is not a handle"))
    }

    fn optional<T>(&self, index: usize) -> Result<Option<Handle<T>>, String> {
        if self.word(index)? == NONE {
            return Ok(None);
        }
        self.handle(index).map(Some)
    }

    fn small(&self, index: usize) -> Result<u32, String> {
        u32::try_from(self.word(index)?).map_err(|_| format!("argument {index} is out of range"))
    }

    fn text(&self, index: usize) -> Result<&str, String> {
        self.texts
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("text argument {index} is missing"))
    }
}

/// One module under construction and the records its next call consumes.
pub(super) struct ShaderBuild {
    builder: ModuleBuilder,
    span: Span,
    /// Operands of the next call that takes a list (section 6.6 bulk
    /// buffers): compose parts, math arguments, call arguments, entries.
    words: Vec<u64>,
    members: Vec<PendingMember>,
    params: Vec<FunctionArgument>,
    groups: Vec<GroupDecl>,
    options: Vec<Handle<Override>>,
}

struct PendingMember {
    name: String,
    ty: Handle<Type>,
    binding: Option<klf_shader::Binding>,
    offset: i64,
    size: u32,
    align: u32,
    span: Span,
}

pub(super) struct PendingFunction {
    module: usize,
    builder: FunctionBuilder,
    workgroup: [WorkgroupExtent; 3],
    sized: bool,
}

#[derive(Default)]
pub(crate) struct ShaderState {
    settings: Option<ShaderSettings>,
    modules: Vec<Option<ShaderBuild>>,
    functions: Vec<Option<PendingFunction>>,
    error: Option<String>,
    /// The byte offset each line of a file starts at, by file id.
    lines: HashMap<u32, Vec<u32>>,
}

impl ShaderState {
    pub(crate) fn new(settings: Option<ShaderSettings>) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    fn build(&mut self, module: u64) -> Result<&mut ShaderBuild, String> {
        usize::try_from(module)
            .ok()
            .and_then(|index| self.modules.get_mut(index))
            .and_then(Option::as_mut)
            .ok_or_else(|| "unknown shader module".to_owned())
    }

    fn push(&mut self, args: &Args) -> Result<u64, String> {
        let word = args.word(1)?;
        self.build(args.word(0)?)?.words.push(word);
        Ok(0)
    }
}

type Service = fn(&mut ShaderState, &Args) -> Result<u64, String>;

/// Every service that needs only the builder state: its symbol, the kind of
/// each argument (`i` a word, `x` a string), and its body.
const SERVICES: &[(&str, &str, Service)] = &[
    ("klfsvc_shader_push", "ii", ShaderState::push),
    ("klfsvc_shader_type", "ix", ShaderState::type_text),
    ("klfsvc_shader_type_array", "iiix", ShaderState::type_array),
    (
        "klfsvc_shader_type_member",
        "ixixiii",
        ShaderState::type_member,
    ),
    ("klfsvc_shader_type_struct", "ixx", ShaderState::type_struct),
    (
        "klfsvc_shader_type_struct_explicit",
        "ixii",
        ShaderState::type_struct_explicit,
    ),
    ("klfsvc_shader_type_size", "iix", ShaderState::type_size),
    ("klfsvc_shader_type_align", "iix", ShaderState::type_align),
    ("klfsvc_shader_constant", "ixii", ShaderState::constant),
    ("klfsvc_shader_override", "ixiii", ShaderState::override_),
    ("klfsvc_shader_global", "ixxiii", ShaderState::global),
    ("klfsvc_shader_slot", "iix", ShaderState::slot),
    ("klfsvc_shader_param", "ixix", ShaderState::param),
    ("klfsvc_shader_function", "ixix", ShaderState::function),
    ("klfsvc_shader_entry", "ixxix", ShaderState::entry),
    (
        "klfsvc_shader_entry_workgroup",
        "iiiii",
        ShaderState::entry_workgroup,
    ),
    (
        "klfsvc_shader_entry_workgroup_override",
        "iiii",
        ShaderState::entry_workgroup_override,
    ),
    (
        "klfsvc_shader_function_finish",
        "ii",
        ShaderState::function_finish,
    ),
    ("klfsvc_shader_group", "ixx", ShaderState::group),
    ("klfsvc_shader_group_add", "ii", ShaderState::group_add),
    ("klfsvc_shader_option", "ii", ShaderState::option),
    ("klfsvc_shader_shader", "ixx", ShaderState::shader),
    (
        "klfsvc_shader_expr_literal",
        "iixx",
        ShaderState::expr_literal,
    ),
    ("klfsvc_shader_expr_zero", "iii", ShaderState::expr_zero),
    (
        "klfsvc_shader_expr_compose",
        "iii",
        ShaderState::expr_compose,
    ),
    ("klfsvc_shader_expr_splat", "iiii", ShaderState::expr_splat),
    (
        "klfsvc_shader_expr_swizzle",
        "iiix",
        ShaderState::expr_swizzle,
    ),
    (
        "klfsvc_shader_expr_access",
        "iiii",
        ShaderState::expr_access,
    ),
    (
        "klfsvc_shader_expr_access_index",
        "iiii",
        ShaderState::expr_access_index,
    ),
    (
        "klfsvc_shader_expr_argument",
        "iii",
        ShaderState::expr_argument,
    ),
    ("klfsvc_shader_expr_global", "iii", ShaderState::expr_global),
    ("klfsvc_shader_expr_local", "iii", ShaderState::expr_local),
    (
        "klfsvc_shader_expr_constant",
        "iii",
        ShaderState::expr_constant,
    ),
    (
        "klfsvc_shader_expr_override",
        "iii",
        ShaderState::expr_override,
    ),
    ("klfsvc_shader_expr_load", "iii", ShaderState::expr_load),
    ("klfsvc_shader_expr_unary", "iixi", ShaderState::expr_unary),
    (
        "klfsvc_shader_expr_binary",
        "iixii",
        ShaderState::expr_binary,
    ),
    (
        "klfsvc_shader_expr_select",
        "iiiii",
        ShaderState::expr_select,
    ),
    (
        "klfsvc_shader_expr_convert",
        "iiix",
        ShaderState::expr_convert,
    ),
    (
        "klfsvc_shader_expr_bitcast",
        "iiii",
        ShaderState::expr_bitcast,
    ),
    ("klfsvc_shader_expr_math", "iix", ShaderState::expr_math),
    (
        "klfsvc_shader_expr_relational",
        "iixi",
        ShaderState::expr_relational,
    ),
    (
        "klfsvc_shader_expr_derivative",
        "iixxi",
        ShaderState::expr_derivative,
    ),
    (
        "klfsvc_shader_expr_image_sample",
        "iix",
        ShaderState::expr_image_sample,
    ),
    (
        "klfsvc_shader_expr_image_load",
        "ii",
        ShaderState::expr_image_load,
    ),
    (
        "klfsvc_shader_expr_image_query",
        "iiixi",
        ShaderState::expr_image_query,
    ),
    (
        "klfsvc_shader_expr_array_length",
        "iii",
        ShaderState::expr_array_length,
    ),
    ("klfsvc_shader_expr_name", "iiix", ShaderState::expr_name),
    ("klfsvc_shader_local", "iixii", ShaderState::local),
    ("klfsvc_shader_stmt_store", "iiii", ShaderState::stmt_store),
    (
        "klfsvc_shader_stmt_image_store",
        "ii",
        ShaderState::stmt_image_store,
    ),
    ("klfsvc_shader_stmt_call", "iii", ShaderState::stmt_call),
    (
        "klfsvc_shader_stmt_atomic",
        "iiixiii",
        ShaderState::stmt_atomic,
    ),
    (
        "klfsvc_shader_stmt_barrier",
        "iixx",
        ShaderState::stmt_barrier,
    ),
    ("klfsvc_shader_stmt_if", "iii", ShaderState::stmt_if),
    ("klfsvc_shader_stmt_else", "ii", ShaderState::stmt_else),
    ("klfsvc_shader_stmt_end_if", "ii", ShaderState::stmt_end_if),
    ("klfsvc_shader_stmt_loop", "ii", ShaderState::stmt_loop),
    (
        "klfsvc_shader_stmt_continuing",
        "ii",
        ShaderState::stmt_continuing,
    ),
    (
        "klfsvc_shader_stmt_end_loop",
        "iii",
        ShaderState::stmt_end_loop,
    ),
    ("klfsvc_shader_stmt_switch", "iii", ShaderState::stmt_switch),
    ("klfsvc_shader_stmt_case", "iixi", ShaderState::stmt_case),
    (
        "klfsvc_shader_stmt_end_case",
        "ii",
        ShaderState::stmt_end_case,
    ),
    (
        "klfsvc_shader_stmt_end_switch",
        "ii",
        ShaderState::stmt_end_switch,
    ),
    ("klfsvc_shader_stmt_break", "ii", ShaderState::stmt_break),
    (
        "klfsvc_shader_stmt_continue",
        "ii",
        ShaderState::stmt_continue,
    ),
    ("klfsvc_shader_stmt_return", "iii", ShaderState::stmt_return),
    ("klfsvc_shader_stmt_kill", "ii", ShaderState::stmt_kill),
];

/// Reads one call's words as `kinds` describes them.
fn decode(kinds: &str, words: &[u64], memory: &mut dyn ServiceMemory) -> Option<Args> {
    let mut args = Args {
        words: Vec::new(),
        texts: Vec::new(),
    };
    let mut at = 0;
    for kind in kinds.bytes() {
        if kind == b'x' {
            let address = *words.get(at)?;
            let length = usize::try_from(*words.get(at + 1)?).ok()?;
            if length > klf_rt::MAX_SERVICE_BUFFER_BYTES {
                return None;
            }
            let mut bytes = vec![0; length];
            if length > 0 && !memory.read_buffer(address, &mut bytes) {
                return None;
            }
            args.texts.push(String::from_utf8(bytes).ok()?);
            at += 2;
        } else {
            args.words.push(*words.get(at)?);
            at += 1;
        }
    }
    (at == words.len()).then_some(args)
}

/// Runs one service, keeping its first misuse.
fn call(state: &mut ShaderState, symbol: &str, body: Service, args: Option<Args>) -> u64 {
    if state.error.is_some() {
        return NONE;
    }
    let result = args
        .ok_or_else(|| "malformed arguments".to_owned())
        .and_then(|args| body(state, &args));
    match result {
        Ok(word) => word,
        Err(message) => {
            let service = symbol.trim_start_matches("klfsvc_").replace('_', ".");
            state.error = Some(format!("`klf.{service}`: {message}"));
            NONE
        }
    }
}

pub(crate) fn register(state: &Arc<Mutex<DefinitionServiceState>>, sink: &mut ServiceSink) {
    for (symbol, kinds, body) in SERVICES {
        sink.register_state_with_memory(*symbol, state, move |state, words, memory| {
            let args = decode(kinds, words, memory);
            call(&mut state.shader, symbol, *body, args)
        });
    }
    sink.register_state_with_memory("klfsvc_shader_module_new", state, |state, words, memory| {
        let args = decode("iiix", words, memory);
        state.shader_module_new(args)
    });
    sink.register_state("klfsvc_shader_module_finish", state, |state, words| {
        state.shader_module_finish(words)
    });
    sink.register_state("klfsvc_shader_span", state, |state, words| {
        state.shader_span(words)
    });
}

impl DefinitionServiceState {
    fn shader_module_new(&mut self, args: Option<Args>) -> u64 {
        let Some(args) = args else {
            return NONE;
        };
        let input = self
            .input_artifact
            .map(|id| u64::try_from(id.index()).unwrap_or(NONE));
        if !self.valid_session_unit(&args.words) || args.words.get(2).copied() != input {
            return NONE;
        }
        let Some(settings) = self.shader.settings.clone() else {
            self.shader.error = Some("the shader builder serves only a ShaderLanguage".to_owned());
            return NONE;
        };
        let name = args.texts.first().cloned().unwrap_or_default();
        let mut builder = ModuleBuilder::new(name, settings.lang, settings.layouts);
        builder.module.capabilities.extend(settings.capabilities);
        if let Some(file) = self.file
            && let Some((_, path, _)) = self.sources.iter().find(|(id, _, _)| *id == file)
        {
            builder.module.file(path);
        }
        self.shader.modules.push(Some(ShaderBuild {
            builder,
            span: Span::default(),
            words: Vec::new(),
            members: Vec::new(),
            params: Vec::new(),
            groups: Vec::new(),
            options: Vec::new(),
        }));
        u64::try_from(self.shader.modules.len() - 1).unwrap_or(NONE)
    }

    fn shader_module_finish(&mut self, words: &[u64]) -> u64 {
        let Some((id, DefinitionArtifactKind::Module)) = self.output else {
            return NONE;
        };
        let taken = words
            .first()
            .and_then(|module| usize::try_from(*module).ok())
            .and_then(|index| self.shader.modules.get_mut(index))
            .and_then(Option::take);
        let open = self.shader.functions.iter().flatten().count();
        match (self.shader.error.take(), taken) {
            (Some(error), _) => self.module_error = Some(misuse(&error)),
            (None, None) => self.module_error = Some(misuse("unknown shader module")),
            (None, Some(_)) if open > 0 => {
                self.module_error = Some(misuse(&format!(
                    "{open} function bodies were never finished"
                )));
            }
            (None, Some(build)) => {
                self.materialized = Some(DefinitionMaterialization::Shader(Box::new(
                    build.builder.finish(),
                )));
            }
        }
        u64::try_from(id.index()).unwrap_or(NONE)
    }

    /// `klf.shader.span(module, file, start, end)`: the span the following
    /// nodes carry. File `-1` is the phase's own.
    fn shader_span(&mut self, words: &[u64]) -> u64 {
        let [module, file, start, end] = words else {
            return NONE;
        };
        let file = match (*file, self.file) {
            (NONE, Some(own)) => own,
            (index, _) => match u32::try_from(index) {
                Ok(index) => klf_core::FileId::from_u32(index),
                Err(_) => return NONE,
            },
        };
        let (Ok(start), Ok(end)) = (u32::try_from(*start), u32::try_from(*end)) else {
            return NONE;
        };
        let Some((_, path, text)) = self.sources.iter().find(|(id, _, _)| *id == file) else {
            return NONE;
        };
        let lines = self
            .shader
            .lines
            .entry(file.as_u32())
            .or_insert_with(|| line_starts(text));
        let line = lines.partition_point(|line_start| *line_start <= start);
        let line_start = lines[line.saturating_sub(1)] as usize;
        let column = text
            .get(line_start..start as usize)
            .map_or(1, |prefix| prefix.chars().count() + 1);
        let path = path.clone();
        let Ok(build) = self.shader.build(*module) else {
            return NONE;
        };
        build.span = Span {
            file: build.builder.module.file(&path),
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(column).unwrap_or(u32::MAX),
            start,
            end,
        };
        0
    }
}

fn line_starts(text: &str) -> Vec<u32> {
    std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| u32::try_from(index + 1).unwrap_or(u32::MAX)),
        )
        .collect()
}

fn misuse(message: &str) -> String {
    format!("KLF-SHD-9001: shader builder misuse: {message}")
}
