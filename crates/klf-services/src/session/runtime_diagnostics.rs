//! Resolve definition diagnostics to their session source files.

use super::{FileId, ResolvedRuntimeDiagnostic, RuntimeDiagnostic, Session};

impl Session {
    /// Resolves and retains one runtime diagnostic through the registered specification.
    pub fn report_runtime_diagnostic(&mut self, file: FileId, record: RuntimeDiagnostic) -> bool {
        self.report_runtime_diagnostics(file, &[record]).is_ok()
    }

    /// Resolves and retains runtime diagnostics atomically.
    ///
    /// # Errors
    /// Returns the first record whose code or span cannot be resolved; no records are appended.
    pub fn report_runtime_diagnostics(
        &mut self, file: FileId, records: &[RuntimeDiagnostic],
    ) -> Result<(), RuntimeDiagnostic> {
        let mut resolved = Vec::with_capacity(records.len());
        for record in records {
            let Some(report) = self.resolve_runtime_diagnostic(file, record.clone()) else {
                return Err(record.clone());
            };
            resolved.push(report);
        }
        self.runtime_diagnostics.extend(resolved);
        Ok(())
    }

    fn resolve_runtime_diagnostic(&self, file: FileId, record: RuntimeDiagnostic) -> Option<ResolvedRuntimeDiagnostic> {
        let file = if record.origin > 0 {
            self.files.iter().find(|source| {
                i64::try_from(klf_source::source_hash(source.text())) == Ok(record.origin)
            })?.id()
        } else {
            match u32::try_from(record.file) {
                Ok(id) => FileId::from_u32(id),
                Err(_) => file,
            }
        };
        let source = self.files.get(file.index())?;
        let end = usize::try_from(record.end).ok()?;
        if end > source.text().len() { return None; }
        self.diagnostic_specs.get(&record.code)?.resolve(record, file)
    }

    /// Describes an unresolved runtime diagnostic and its intended location.
    #[must_use]
    pub fn describe_unresolved(&self, record: RuntimeDiagnostic) -> String {
        let title = self.diagnostic_specs.get(&record.code)
            .map_or_else(|| "unregistered diagnostic".to_owned(), |spec| spec.title.clone());
        let file = if record.origin > 0 {
            self.files.iter().find(|source| i64::try_from(klf_source::source_hash(source.text())) == Ok(record.origin))
        } else {
            u32::try_from(record.file).ok().and_then(|id| self.files.get(FileId::from_u32(id).index()))
        };
        let place = match file {
            Some(source) => format!("{} (bytes {}..{} of {})", source.name().as_str(), record.start, record.end, source.text().len()),
            None if record.origin > 0 => format!("a source the session does not hold (content hash {}), bytes {}..{}", record.origin, record.start, record.end),
            None => format!("bytes {}..{}", record.start, record.end),
        };
        format!("error[{}]: {title}, at {place}", record.code)
    }
}
