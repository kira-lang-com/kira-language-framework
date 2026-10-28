//! The descriptor blob a natively compiled frontend carries in its own binary.
//!
//! A native frontend has no `.klfd` package to read at startup: the driver
//! behaviour section 5.2 asks for — the tool's name, accepted extensions,
//! file modes, flag defaults and diagnostics — has to travel with the code.
//! `klf build` encodes the `Language` descriptor here and the runtime decodes
//! it back out of `klffe_metadata`.
//!
//! One record per line, fields separated by tabs, with `\t`, `\n` and `\\`
//! escaped in values so every string field round-trips.

use klf_diag::Severity;
use klf_services::{
    DefaultConvention, DiagnosticSpec, DriverFlag, DriverFlagValue, EntryConvention, Exclusivity,
    FileMode, LanguageDescriptor, MemoryModel, ShaderFamily,
};
use klf_shader::builder::LayoutDefaults;
use klf_shader::{Capability, LayoutRule, Target};

/// An optional hook a definition declares beyond its phases.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Hook {
    /// A `compatDriver` override.
    CompatDriver,
    /// A `runProgram` hook.
    RunProgram,
    /// A shader `emit` hook.
    Emit,
    /// A `format` hook.
    Format,
    /// A `project` hook.
    Project,
}

impl Hook {
    pub const ALL: [Self; 5] = [
        Self::CompatDriver,
        Self::RunProgram,
        Self::Emit,
        Self::Format,
        Self::Project,
    ];

    /// The hook's key in the encoded metadata.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::CompatDriver => "compat_driver",
            Self::RunProgram => "run_program",
            Self::Emit => "emit",
            Self::Format => "format",
            Self::Project => "project",
        }
    }
}

/// What `klf build` recorded about a definition package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontendMetadata {
    pub name: String,
    pub driver: String,
    pub version: String,
    /// SHA-256 over the definition package's sources, in hex: the definition
    /// hash a cache key carries (section 5.6).
    pub definition: String,
    pub target: String,
    /// The definition's `stdRoot` as an absolute path when the frontend was
    /// built, where a frontend outside a sysroot finds the std's sources.
    pub std_root: String,
    pub extensions: Vec<String>,
    pub memory: MemoryModel,
    pub exclusivity: Exclusivity,
    pub convention: DefaultConvention,
    pub entry: EntryConvention,
    pub file_modes: Vec<FileMode>,
    pub flags: Vec<DriverFlag>,
    pub diagnostics: Vec<DiagnosticSpec>,
    /// The optional hooks the definition declares, in [`Hook::ALL`] order.
    pub hooks: Vec<Hook>,
    /// Whether each phase, in `units`/`parse`/`elaborate`/`lower` order, ends
    /// by materializing its artifact. The VM-hosted driver works this out from
    /// the call graph; natively there is no call graph left, so it is decided
    /// at build time and carried here.
    pub materializes: [bool; 4],
    /// The declared editor spec; absent means `EditorSpec.minimal(name)`.
    pub editor: Option<klf_facts::spec::EditorSpec>,
    /// The `ShaderLanguage` members, for a shader frontend.
    pub shader: Option<ShaderFamily>,
    /// The definition's own command line (`KLF-CLI.md` section 3).
    pub cli: klf_args::Command,
}

impl Default for FrontendMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            driver: String::new(),
            version: String::new(),
            definition: String::new(),
            target: String::new(),
            std_root: String::new(),
            extensions: Vec::new(),
            memory: MemoryModel::Counted,
            exclusivity: Exclusivity::None,
            convention: DefaultConvention::Platform,
            entry: EntryConvention::Outcome,
            file_modes: Vec::new(),
            flags: Vec::new(),
            diagnostics: Vec::new(),
            hooks: Vec::new(),
            materializes: [false; 4],
            editor: None,
            shader: None,
            cli: klf_args::Command::default(),
        }
    }
}

impl FrontendMetadata {
    #[must_use]
    pub fn from_descriptor(descriptor: &LanguageDescriptor, target: &str) -> Self {
        Self {
            name: descriptor.name.clone(),
            driver: descriptor.driver.clone(),
            version: descriptor.version.clone(),
            definition: String::new(),
            target: target.to_owned(),
            std_root: String::new(),
            extensions: descriptor.extensions.clone(),
            memory: descriptor.memory,
            exclusivity: descriptor.exclusivity,
            convention: descriptor.convention,
            entry: descriptor.entry,
            file_modes: descriptor.file_modes.clone(),
            flags: descriptor.flags.clone(),
            diagnostics: descriptor.diagnostics.clone(),
            hooks: Vec::new(),
            materializes: [false; 4],
            editor: descriptor.editor.clone(),
            shader: descriptor.shader.clone(),
            cli: descriptor.cli.clone(),
        }
    }

    /// Whether the definition declares `hook`.
    #[must_use]
    pub fn declares(&self, hook: Hook) -> bool {
        self.hooks.contains(&hook)
    }

    /// Records that the definition declares `hook`.
    pub fn declare(&mut self, hook: Hook) {
        if !self.declares(hook) {
            self.hooks.push(hook);
            self.hooks.sort_unstable();
        }
    }

    /// The editor spec, `EditorSpec.minimal(name)` when none is declared.
    #[must_use]
    pub fn editor_spec(&self) -> klf_facts::spec::EditorSpec {
        self.editor
            .clone()
            .unwrap_or_else(|| klf_facts::spec::EditorSpec::minimal(&self.name))
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        for (key, value) in [
            ("name", &self.name),
            ("driver", &self.driver),
            ("version", &self.version),
            ("definition", &self.definition),
            ("target", &self.target),
            ("std_root", &self.std_root),
        ] {
            out.push_str(key);
            out.push('\t');
            out.push_str(&escape(value));
            out.push('\n');
        }
        out.push_str("memory_default\t");
        out.push_str(memory_model_key(Some(self.memory)));
        out.push('\n');
        out.push_str("convention_default\t");
        out.push_str(default_convention_key(Some(self.convention)));
        out.push('\n');
        out.push_str("entry\t");
        out.push_str(match self.entry {
            EntryConvention::Outcome => "outcome",
            EntryConvention::Process => "process",
            EntryConvention::ProcessExit => "process_exit",
        });
        out.push('\n');
        out.push_str("exclusivity_default\t");
        out.push_str(exclusivity_key(self.exclusivity));
        out.push('\n');
        for hook in Hook::ALL {
            out.push_str(hook.key());
            out.push('\t');
            out.push_str(if self.declares(hook) { "1" } else { "0" });
            out.push('\n');
        }
        for extension in &self.extensions {
            out.push_str("ext\t");
            out.push_str(&escape(extension));
            out.push('\n');
        }
        for mode in &self.file_modes {
            out.push_str("file_mode\t");
            out.push_str(&escape(&mode.name));
            out.push('\t');
            out.push_str(&escape(&mode.pragma));
            out.push('\t');
            out.push_str(&escape(&mode.flag));
            out.push('\t');
            out.push_str(memory_model_key(mode.memory));
            out.push('\t');
            out.push_str(default_convention_key(mode.convention));
            for extension in &mode.extensions {
                out.push('\t');
                out.push_str(&escape(extension));
            }
            out.push('\n');
        }
        for flag in &self.flags {
            encode_flag(&mut out, flag);
        }
        for spec in &self.diagnostics {
            out.push_str("diag\t");
            out.push_str(&spec.code.to_string());
            out.push('\t');
            out.push_str(severity_key(spec.severity));
            out.push('\t');
            out.push_str(&escape(&spec.title));
            out.push('\t');
            out.push_str(&escape(&spec.explanation));
            out.push('\t');
            out.push_str(&escape(&spec.help));
            out.push('\n');
        }
        if let Some(editor) = &self.editor {
            out.push_str("editor\t");
            out.push_str(&escape(&editor.encode()));
            out.push('\n');
        }
        if let Some(shader) = &self.shader {
            encode_shader(&mut out, shader);
        }
        out.push_str("cli\t");
        out.push_str(&escape(&klf_args::codec::encode(&self.cli)));
        out.push('\n');
        out.push_str("materializes");
        for flag in self.materializes {
            out.push('\t');
            out.push(if flag { '1' } else { '0' });
        }
        out.push('\n');
        out
    }

    /// Reads back what [`Self::encode`] wrote. Unknown records are ignored so a
    /// newer builder can add fields without breaking an older runtime.
    #[must_use]
    pub fn decode(text: &str) -> Self {
        let mut metadata = Self::default();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let Some(key) = fields.next() else { continue };
            match key {
                "name" => metadata.name = unescape(fields.next().unwrap_or_default()),
                "driver" => metadata.driver = unescape(fields.next().unwrap_or_default()),
                "version" => metadata.version = unescape(fields.next().unwrap_or_default()),
                "definition" => metadata.definition = unescape(fields.next().unwrap_or_default()),
                "target" => metadata.target = unescape(fields.next().unwrap_or_default()),
                "std_root" => metadata.std_root = unescape(fields.next().unwrap_or_default()),
                "compat_driver" | "run_program" | "emit" | "format" | "project" => {
                    let hook = Hook::ALL.into_iter().find(|hook| hook.key() == key);
                    if let (Some(hook), Some("1")) = (hook, fields.next()) {
                        metadata.declare(hook);
                    }
                }
                "entry" => match fields.next() {
                    Some("process") => metadata.entry = EntryConvention::Process,
                    Some("process_exit") => metadata.entry = EntryConvention::ProcessExit,
                    _ => {}
                },
                "memory_default" => {
                    if let Some(memory) = memory_model_of(fields.next().unwrap_or_default()) {
                        metadata.memory = memory;
                    }
                }
                "convention_default" => {
                    if let Some(convention) =
                        default_convention_of(fields.next().unwrap_or_default())
                    {
                        metadata.convention = convention;
                    }
                }
                "exclusivity_default" => {
                    if let Some(exclusivity) = exclusivity_of(fields.next().unwrap_or_default()) {
                        metadata.exclusivity = exclusivity;
                    }
                }
                "ext" => metadata
                    .extensions
                    .push(unescape(fields.next().unwrap_or_default())),
                "file_mode" => metadata.file_modes.extend(decode_file_mode(fields)),
                "flag" => metadata.flags.extend(decode_flag(fields)),
                "cli" => {
                    if let Some(cli) =
                        klf_args::codec::decode(&unescape(fields.next().unwrap_or_default()))
                    {
                        metadata.cli = cli;
                    }
                }
                "diag" => {
                    let code = fields
                        .next()
                        .and_then(|field| field.parse::<i64>().ok())
                        .unwrap_or_default();
                    let severity = severity_of(fields.next().unwrap_or_default());
                    let title = unescape(fields.next().unwrap_or_default());
                    let explanation = unescape(fields.next().unwrap_or_default());
                    // Records written before the help field have none.
                    let help = unescape(fields.next().unwrap_or_default());
                    metadata.diagnostics.push(DiagnosticSpec {
                        code,
                        severity,
                        title,
                        explanation,
                        help,
                    });
                }
                "editor" => {
                    metadata.editor = klf_facts::spec::EditorSpec::decode(&unescape(
                        fields.next().unwrap_or_default(),
                    ))
                    .ok();
                }
                "shader" => metadata.shader = decode_shader(fields),
                "shader_key" => {
                    let key = fields.next().unwrap_or_default().to_owned();
                    let code = fields.next().and_then(|code| code.parse().ok());
                    if let (Some(shader), Some(code)) = (&mut metadata.shader, code) {
                        shader.keys.push((key, code));
                    }
                }
                "shader_capability" => {
                    let capability = fields.next().and_then(Capability::from_name);
                    if let (Some(shader), Some(capability)) = (&mut metadata.shader, capability) {
                        shader.capabilities.push(capability);
                    }
                }
                "materializes" => {
                    for slot in &mut metadata.materializes {
                        *slot = fields.next() == Some("1");
                    }
                }
                _ => {}
            }
        }
        metadata
    }
}

/// `shader`, the three layout rules, the library root and the targets.
fn encode_shader(out: &mut String, shader: &ShaderFamily) {
    out.push_str("shader");
    for rule in [
        shader.layouts.uniform,
        shader.layouts.storage,
        shader.layouts.immediate,
    ] {
        out.push('\t');
        out.push_str(rule.name());
    }
    out.push('\t');
    out.push_str(&escape(&shader.library_root));
    for target in &shader.targets {
        out.push('\t');
        out.push_str(target.name());
    }
    out.push('\n');
    for (key, code) in &shader.keys {
        out.push_str("shader_key\t");
        out.push_str(key);
        out.push('\t');
        out.push_str(&code.to_string());
        out.push('\n');
    }
    for capability in &shader.capabilities {
        out.push_str("shader_capability\t");
        out.push_str(capability.name());
        out.push('\n');
    }
}

fn decode_shader(mut fields: std::str::Split<'_, char>) -> Option<ShaderFamily> {
    let mut rule = || {
        let name = fields.next()?;
        [
            LayoutRule::Std140,
            LayoutRule::Std430,
            LayoutRule::Scalar,
            LayoutRule::Msl,
            LayoutRule::Explicit,
        ]
        .into_iter()
        .find(|rule| rule.name() == name)
    };
    let layouts = LayoutDefaults {
        uniform: rule()?,
        storage: rule()?,
        immediate: rule()?,
    };
    let library_root = unescape(fields.next()?);
    let targets = fields.map(Target::from_name).collect::<Option<_>>()?;
    Some(ShaderFamily {
        layouts,
        targets,
        library_root,
        keys: Vec::new(),
        capabilities: Vec::new(),
    })
}

const fn memory_model_key(memory: Option<MemoryModel>) -> &'static str {
    match memory {
        None => "",
        Some(MemoryModel::Unmanaged) => "unmanaged",
        Some(MemoryModel::Counted) => "counted",
        Some(MemoryModel::Affine) => "affine",
    }
}

fn encode_flag(out: &mut String, flag: &DriverFlag) {
    out.push_str("flag\t");
    out.push_str(&escape(&flag.name));
    match &flag.default_value {
        DriverFlagValue::Boolean(value) => {
            out.push_str(if *value {
                "\tboolean\t1"
            } else {
                "\tboolean\t0"
            });
        }
        DriverFlagValue::Integer(value) => {
            out.push_str("\tinteger\t");
            out.push_str(&value.to_string());
        }
        DriverFlagValue::String(value) => {
            out.push_str("\tstring\t");
            out.push_str(&escape(value));
        }
    }
    out.push('\n');
}

fn decode_file_mode(mut fields: std::str::Split<'_, char>) -> Option<FileMode> {
    let (name, pragma, flag, memory, convention) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    let memory = if memory.is_empty() {
        None
    } else {
        Some(memory_model_of(memory)?)
    };
    let convention = if convention.is_empty() {
        None
    } else {
        Some(default_convention_of(convention)?)
    };
    Some(FileMode {
        name: unescape(name),
        pragma: unescape(pragma),
        flag: unescape(flag),
        extensions: fields.map(unescape).collect(),
        memory,
        convention,
    })
}

fn decode_flag(mut fields: std::str::Split<'_, char>) -> Option<DriverFlag> {
    let (name, kind, value) = (fields.next()?, fields.next()?, fields.next()?);
    let default_value = match kind {
        "boolean" => match value {
            "0" => DriverFlagValue::Boolean(false),
            "1" => DriverFlagValue::Boolean(true),
            _ => return None,
        },
        "integer" => DriverFlagValue::Integer(value.parse().ok()?),
        "string" => DriverFlagValue::String(unescape(value)),
        _ => return None,
    };
    Some(DriverFlag {
        name: unescape(name),
        default_value,
    })
}

fn memory_model_of(key: &str) -> Option<MemoryModel> {
    match key {
        "unmanaged" => Some(MemoryModel::Unmanaged),
        "counted" => Some(MemoryModel::Counted),
        "affine" => Some(MemoryModel::Affine),
        _ => None,
    }
}

const fn default_convention_key(convention: Option<DefaultConvention>) -> &'static str {
    match convention {
        None => "",
        Some(DefaultConvention::Ridl) => "ridl",
        Some(DefaultConvention::Platform) => "platform",
    }
}

fn default_convention_of(key: &str) -> Option<DefaultConvention> {
    match key {
        "ridl" => Some(DefaultConvention::Ridl),
        "platform" => Some(DefaultConvention::Platform),
        _ => None,
    }
}

const fn exclusivity_key(exclusivity: Exclusivity) -> &'static str {
    match exclusivity {
        Exclusivity::Static => "static",
        Exclusivity::Dynamic => "dynamic",
        Exclusivity::None => "none",
    }
}

fn exclusivity_of(key: &str) -> Option<Exclusivity> {
    match key {
        "static" => Some(Exclusivity::Static),
        "dynamic" => Some(Exclusivity::Dynamic),
        "none" => Some(Exclusivity::None),
        _ => None,
    }
}

const fn severity_key(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Remark => "remark",
    }
}

fn severity_of(key: &str) -> Severity {
    match key {
        "warning" => Severity::Warning,
        "note" => Severity::Note,
        "remark" => Severity::Remark,
        _ => Severity::Error,
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_metadata_round_trips_modes_and_typed_flag_defaults() {
        let file_modes = vec![
            FileMode {
                name: "legacy\tmode\n\\name".to_owned(),
                pragma: "legacy\npragma".to_owned(),
                flag: "--legacy\tmode".to_owned(),
                extensions: vec![".old".to_owned(), ".legacy\tcpp".to_owned()],
                memory: Some(MemoryModel::Affine),
                convention: Some(DefaultConvention::Ridl),
            },
            FileMode {
                name: "modern".to_owned(),
                pragma: "modern".to_owned(),
                flag: "--modern".to_owned(),
                extensions: Vec::new(),
                memory: Some(MemoryModel::Unmanaged),
                convention: Some(DefaultConvention::Platform),
            },
            FileMode {
                name: "inherited".to_owned(),
                pragma: String::new(),
                flag: String::new(),
                extensions: vec![".inherit".to_owned()],
                memory: None,
                convention: None,
            },
        ];
        let descriptor = LanguageDescriptor::new(
            "test",
            "test",
            "0.1.0",
            vec![".test".to_owned()],
            MemoryModel::Affine,
            Exclusivity::Dynamic,
            DefaultConvention::Ridl,
            "",
        )
        .with_file_modes(file_modes.clone())
        .with_flags(vec![
            DriverFlag {
                name: "warnings".to_owned(),
                default_value: DriverFlagValue::Boolean(false),
            },
            DriverFlag {
                name: "jobs".to_owned(),
                default_value: DriverFlagValue::Integer(4),
            },
            DriverFlag {
                name: "sysroot".to_owned(),
                default_value: DriverFlagValue::String("std\troot".to_owned()),
            },
        ])
        .with_editor(Some(editor_spec()));
        let mut metadata = FrontendMetadata::from_descriptor(&descriptor, "test-target");
        metadata.declare(Hook::Emit);
        metadata.declare(Hook::CompatDriver);
        metadata.declare(Hook::RunProgram);
        metadata.std_root = "/definitions/kira/std".to_owned();

        assert_eq!(metadata.file_modes, file_modes);
        assert_eq!(metadata.memory, MemoryModel::Affine);
        assert_eq!(metadata.exclusivity, Exclusivity::Dynamic);
        assert_eq!(metadata.convention, DefaultConvention::Ridl);
        assert_eq!(metadata.flags, descriptor.flags);
        assert_eq!(FrontendMetadata::decode(&metadata.encode()), metadata);
        let old = FrontendMetadata::decode("name\tlegacy\n");
        assert_eq!(old.memory, MemoryModel::Counted);
        assert_eq!(old.exclusivity, Exclusivity::None);
        assert_eq!(old.convention, DefaultConvention::Platform);
        assert_eq!(old.file_modes, Vec::<FileMode>::new());
        assert_eq!(old.flags, Vec::<DriverFlag>::new());
        assert_eq!(
            old.editor_spec(),
            klf_facts::spec::EditorSpec::minimal("legacy")
        );
    }

    fn editor_spec() -> klf_facts::spec::EditorSpec {
        use klf_facts::spec::{BracketPair, EnterAction, GrammarRef, KeywordGroup, OnEnterRule};
        use klf_facts::{TokenClass, TokenModifier};
        let mut spec = klf_facts::spec::EditorSpec::minimal("test");
        spec.root_markers = vec!["kira.toml".to_owned()];
        spec.strings[0].interpolation = Some(BracketPair {
            open: "\\(".to_owned(),
            close: ")".to_owned(),
        });
        spec.keywords = vec![KeywordGroup {
            class: TokenClass::Keyword,
            modifiers: vec![TokenModifier::Control, TokenModifier::Contextual],
            words: vec!["if".to_owned(), "else".to_owned()],
        }];
        spec.on_enter = vec![OnEnterRule {
            before: "^\\s*///".to_owned(),
            after: None,
            action: EnterAction::None,
            append: "/// ".to_owned(),
        }];
        spec.region_markers = Some(("// region\t".to_owned(), "// endregion\n".to_owned()));
        spec.grammar = Some(GrammarRef {
            repository: "https://example.org/grammar".to_owned(),
            revision: "abc".to_owned(),
            path: String::new(),
        });
        spec
    }
}
