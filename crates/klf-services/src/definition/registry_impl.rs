use super::*;

impl DefinitionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a definition after validating its metadata.
    ///
    /// # Errors
    /// Returns [`DefinitionRegistryError`] when its name or an extension is
    /// already claimed by a registered definition, or repeated in its own
    /// metadata.
    pub fn register<D: Definition + 'static>(
        &mut self,
        definition: D,
    ) -> Result<(), DefinitionRegistryError> {
        self.register_box(Box::new(definition))
    }

    /// Registers a boxed definition after validating its metadata.
    ///
    /// # Errors
    /// Returns [`DefinitionRegistryError`] when its name or an extension is
    /// already claimed by a registered definition, or repeated in its own
    /// metadata.
    pub fn register_box(
        &mut self,
        definition: Box<dyn Definition>,
    ) -> Result<(), DefinitionRegistryError> {
        let language = definition.language();
        if self
            .definitions
            .iter()
            .any(|existing| existing.language().name == language.name)
        {
            return Err(DefinitionRegistryError::DuplicateName {
                name: language.name,
            });
        }

        let mut extensions = HashSet::new();
        for extension in &language.extensions {
            if !extensions.insert(extension) {
                return Err(DefinitionRegistryError::DuplicateExtension {
                    extension: extension.clone(),
                    first: language.name.clone(),
                    second: language.name,
                });
            }
            if let Some(existing) = self
                .definitions
                .iter()
                .find(|existing| existing.language().claims_extension(extension))
            {
                return Err(DefinitionRegistryError::DuplicateExtension {
                    extension: extension.clone(),
                    first: existing.language().name,
                    second: language.name,
                });
            }
        }

        self.definitions.push(definition);
        Ok(())
    }

    #[must_use]
    pub fn select(&self, file_name: &str) -> Option<&dyn Definition> {
        let extension = std::path::Path::new(file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        self.definitions
            .iter()
            .find(|definition| definition.language().claims_extension(extension))
            .map(Box::as_ref)
    }

    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&dyn Definition> {
        self.definitions
            .iter()
            .find(|definition| definition.language().name == name)
            .map(Box::as_ref)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}
