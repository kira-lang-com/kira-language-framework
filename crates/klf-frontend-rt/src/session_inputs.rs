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
    let overlay = session.overlay();
    let inputs = expand_build_inputs(&invocation.inputs, metadata)?;
    for path in &inputs {
        let overlaid = overlay.contains_key(&klf_services::overlay_key(path));
        let before = (!overlaid)
            .then(|| klf_services::file_identity(path))
            .flatten();
        let text = session
            .read_text(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let after = (!overlaid)
            .then(|| klf_services::file_identity(path))
            .flatten();
        let file_identity = before.filter(|before| Some(before) == after.as_ref());
        let selected_mode = select_file_mode(
            &metadata.file_modes,
            path,
            &text,
            &invocation.file_mode_globs,
        )
        .map_err(|error| format!("KLF-DRV-0022: {}: {error}", path.display()))?;
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        known_sources.insert(canonical);
        let file = session.add_file_with_identity(
            path.to_string_lossy().as_ref(),
            text.clone(),
            file_identity,
        );
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

/// A directory with a recognized source file among its declared root markers
/// can seed compilation from that file. The language loader follows imports
/// and discovers the rest of the compilation graph, so expanding every source
/// beneath the directory first would only duplicate filesystem work.
fn expand_build_inputs(
    inputs: &[PathBuf],
    metadata: &FrontendMetadata,
) -> Result<Vec<PathBuf>, String> {
    if inputs.len() == 1 {
        if let Some(editor) = &metadata.editor {
            let input = &inputs[0];
            if std::fs::metadata(input)
                .ok()
                .is_some_and(|metadata| metadata.is_dir())
            {
                for marker in &editor.root_markers {
                    let seed = input.join(marker);
                    let recognized_source = seed
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| {
                            metadata.extensions.iter().any(|known| known == extension)
                        });
                    if recognized_source
                        && std::fs::metadata(&seed)
                            .ok()
                            .is_some_and(|metadata| metadata.is_file())
                    {
                        return Ok(vec![seed]);
                    }
                }
            }
        }
    }
    expand_inputs(inputs, &metadata.extensions)
}

/// The input files: a file as given, and a directory as every file under it
/// whose extension is one the language accepts, in path order (section 5.5
/// `[paths]`). The definition decides what the files mean together.
pub(crate) fn expand_inputs(
    inputs: &[PathBuf],
    extensions: &[String],
) -> Result<Vec<PathBuf>, String> {
    fn walk(
        directory: &Path,
        metadata: Option<&std::fs::Metadata>,
        extensions: &[String],
        found: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        klf_services::note_access(directory);
        let index = klf_services::shared_directory_index();
        let snapshot = metadata
            .map_or_else(
                || index.snapshot(directory),
                |metadata| index.snapshot_with_metadata(directory, metadata),
            )
            .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        for path in snapshot.paths(directory) {
            let child_metadata = std::fs::metadata(&path).ok();
            if child_metadata
                .as_ref()
                .is_some_and(std::fs::Metadata::is_dir)
            {
                walk(&path, child_metadata.as_ref(), extensions, found)?;
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
        let metadata = std::fs::metadata(input).ok();
        if !metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) {
            expanded.push(input.clone());
            continue;
        }
        let mut found = Vec::new();
        walk(input, metadata.as_ref(), extensions, &mut found)?;
        if found.is_empty() {
            return Err(format!("{} holds no source files", input.display()));
        }
        found.sort();
        expanded.extend(found);
    }
    Ok(expanded)
}
