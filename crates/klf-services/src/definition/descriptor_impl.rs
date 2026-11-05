use super::*;

impl LanguageDescriptor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        driver: impl Into<String>,
        version: impl Into<String>,
        extensions: Vec<String>,
        memory: MemoryModel,
        exclusivity: Exclusivity,
        convention: DefaultConvention,
        std_root: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            driver: driver.into(),
            version: version.into(),
            extensions,
            memory,
            exclusivity,
            convention,
            std_root: std_root.into(),
            entry: EntryConvention::Outcome,
            file_modes: Vec::new(),
            flags: Vec::new(),
            diagnostics: Vec::new(),
            editor: None,
            shader: None,
            cli: klf_args::Command::default(),
        }
    }

    #[must_use]
    pub fn with_cli(mut self, cli: klf_args::Command) -> Self {
        self.cli = cli;
        self
    }

    #[must_use]
    pub fn with_shader(mut self, shader: Option<crate::ShaderFamily>) -> Self {
        self.shader = shader;
        self
    }

    #[must_use]
    pub fn with_editor(mut self, editor: Option<klf_facts::spec::EditorSpec>) -> Self {
        self.editor = editor;
        self
    }

    /// The editor spec, `EditorSpec.minimal(name)` when none is declared.
    #[must_use]
    pub fn editor_spec(&self) -> klf_facts::spec::EditorSpec {
        self.editor
            .clone()
            .unwrap_or_else(|| klf_facts::spec::EditorSpec::minimal(&self.name))
    }

    #[must_use]
    pub fn with_file_modes(mut self, file_modes: Vec<FileMode>) -> Self {
        self.file_modes = file_modes;
        self
    }

    #[must_use]
    pub fn with_entry(mut self, entry: EntryConvention) -> Self {
        self.entry = entry;
        self
    }

    #[must_use]
    pub fn with_flags(mut self, flags: Vec<DriverFlag>) -> Self {
        self.flags = flags;
        self
    }

    #[must_use]
    pub fn file_mode(&self, name: &str) -> Option<&FileMode> {
        self.file_modes.iter().find(|mode| mode.name == name)
    }

    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Vec<DiagnosticSpec>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    #[must_use]
    pub fn minimal(name: &str, extensions: &[&str]) -> Self {
        Self::new(
            name,
            name,
            "0.1.0",
            extensions
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            MemoryModel::Counted,
            Exclusivity::None,
            DefaultConvention::Platform,
            "",
        )
    }

    #[must_use]
    pub fn claims_extension(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|candidate| candidate == extension)
    }
}
