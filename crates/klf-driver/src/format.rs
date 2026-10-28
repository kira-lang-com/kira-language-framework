//! Formatting a file with a definition's `format` hook on the VM
//! (`KLF-FORMAT.md` section 5).

use crate::LanguageDefinition;
use klf_defsvc::{FormatFailure, format_file};
use klf_format::{Options, Syntax};
use klf_services::{DefinitionUnit, Session};

impl LanguageDefinition {
    /// Whether the definition declares a `format` hook.
    #[must_use]
    pub fn formats(&self) -> bool {
        self.metadata().entrypoints.format.is_some()
    }

    /// Formats `text`, read from the file `name`, and answers the formatted
    /// text. `session` must be fresh for this file.
    ///
    /// # Errors
    /// Returns [`FormatFailure`] when the definition has no hook, the file
    /// does not parse, or the layout is rejected.
    pub fn format_source(
        &self,
        session: &mut Session,
        name: &str,
        text: &str,
        options: &Options,
    ) -> Result<String, FormatFailure> {
        if !self.formats() {
            return Err(FormatFailure::Declined(format!(
                "definition `{}` has no format hook",
                self.descriptor().name
            )));
        }
        let syntax = Syntax::from_spec(&self.descriptor().editor_spec());
        session
            .initialize_flags(&self.descriptor().flags)
            .map_err(FormatFailure::Declined)?;
        let file = session.add_file(name, text.to_owned());
        session.register_unit(DefinitionUnit::new(file));
        session.register_diagnostics(self.descriptor().diagnostics.clone());
        let program = self.phase_program().map_err(FormatFailure::Run)?;
        let image = self.image();
        let mut make_runner = || Ok(program.host(image));
        format_file(session, file, &syntax, options, &mut make_runner)
    }
}
