//! A frontend's session and the input files it is built from.

use crate::{FrontendMetadata, arguments, output, source_closure};
use klf_core::FileId;
use klf_services::{
    DefinitionUnit, LanguageDefaults, Session, ShaderSettings, effective_file_defaults,
    select_file_mode,
};
use klf_target::Target;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A session for `invocation`, before any input is added.
pub(crate) fn new_session(
    metadata: &FrontendMetadata,
    invocation: &arguments::Invocation,
    triple: &str,
) -> Result<Session, String> {
    let target =
        Target::from_str(triple).map_err(|error| format!("target `{triple}`: {error:?}"))?;
    let mut session = Session::new(target);
    session.initialize_flags(&metadata.flags)?;
    session.set_emit(&invocation.emit);
    session.set_std_root(output::std_root(metadata));
    session.set_language_defaults(LanguageDefaults {
        memory: metadata.memory,
        convention: metadata.convention,
        exclusivity: metadata.exclusivity,
        file_modes: metadata.file_modes.clone(),
        flag_globs: invocation.file_mode_globs.clone(),
    });
    for (name, value) in &invocation.flag_overrides {
        session.set_flag(name, value.clone())?;
    }
    if let Some(shader) = &metadata.shader {
        session.set_shader(ShaderSettings {
            lang: metadata.name.clone(),
            layouts: shader.layouts,
            capabilities: shader.capabilities.clone(),
        });
    }
    session.register_diagnostics(metadata.diagnostics.clone());
    Ok(session)
}

/// Adds the invocation's inputs, and the files they include, to `session`,
/// reading each through the session's overlay. Returns the inputs' files.
pub(crate) fn add_inputs(
    session: &mut Session,
    metadata: &FrontendMetadata,
    invocation: &arguments::Invocation,
) -> Result<Vec<FileId>, String> {
    let mut files: Vec<FileId> = Vec::new();
    let mut known_sources = HashSet::new();
    let inputs = expand_inputs(&invocation.inputs, &metadata.extensions)?;
    for path in &inputs {
        let text = session
            .read_text(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let selected_mode = select_file_mode(
            &metadata.file_modes,
            path,
            &text,
            &invocation.file_mode_globs,
        )
        .map_err(|error| format!("KLF-DRV-0022: {}: {error}", path.display()))?;
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        known_sources.insert(identity);
        let file = session.add_file(path.to_string_lossy().as_ref(), text.clone());
        let (memory, convention) =
            effective_file_defaults(metadata.memory, metadata.convention, selected_mode);
        session.set_file_defaults(file, memory, convention)?;
        session.set_file_exclusivity(file, metadata.exclusivity)?;
        source_closure::register(session, metadata, path, &text, &mut known_sources)?;
        if let Some(selected_mode) = selected_mode {
            let index = metadata
                .file_modes
                .iter()
                .position(|mode| std::ptr::eq(mode, selected_mode))
                .ok_or_else(|| "selected file mode disappeared".to_owned())?;
            session.set_file_mode(file, index)?;
        }
        session.register_unit(DefinitionUnit::new(file));
        files.push(file);
    }
    Ok(files)
}

/// The input files: a file as given, and a directory as every file under it
/// whose extension is one the language accepts, in path order (section 5.5
/// `[paths]`). The definition decides what the files mean together.
pub(crate) fn expand_inputs(inputs: &[PathBuf], extensions: &[String]) -> Result<Vec<PathBuf>, String> {
    fn walk(
        directory: &Path,
        extensions: &[String],
        found: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        klf_services::note_access(directory);
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
                .path();
            if path.is_dir() {
                walk(&path, extensions, found)?;
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.iter().any(|known| known == extension))
            {
                found.push(path);
            }
        }
        Ok(())
    }
    let mut expanded = Vec::new();
    for input in inputs {
        if !input.is_dir() {
            expanded.push(input.clone());
            continue;
        }
        let mut found = Vec::new();
        walk(input, extensions, &mut found)?;
        if found.is_empty() {
            return Err(format!("{} holds no source files", input.display()));
        }
        found.sort();
        expanded.extend(found);
    }
    Ok(expanded)
}
