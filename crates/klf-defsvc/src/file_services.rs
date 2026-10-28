//! `Session.read` (section 8.1) and its directory form: a phase reads source
//! files the driver was not given, such as a package manifest or the sources of
//! a library the program imports.
//!
//! A file read during a phase joins the phase's sources at once, under the
//! `FileId` the Session will give it, so the phase can read it back and report
//! diagnostics in it. [`DefinitionServiceBridge::commit_records`] registers it in
//! the Session when the phase returns, before its diagnostics are resolved.
//!
//! [`DefinitionServiceBridge::commit_records`]: crate::DefinitionServiceBridge::commit_records

use crate::{DefinitionServiceState, ServiceSink};
use klf_core::FileId;
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory};
use klf_services::{Observation, Session};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MISSING: u64 = u64::MAX;

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state_with_memory("klfsvc_source_read", state, read);
    services.register_state_with_memory("klfsvc_source_path_exists", state, path_exists);
    services.register_state_with_memory("klfsvc_source_virtual", state, virtual_source);
    services.register_state_with_memory("klfsvc_source_expansion", state, expansion);
    services.register_state_with_memory("klfsvc_source_ffi_header", state, ffi_header);
    services.register_state_with_memory("klfsvc_source_ridl_module", state, ridl_module);
    services.register_state_with_memory("klfsvc_source_directory", state, directory);
    services.register_state("klfsvc_source_directory_file", state, directory_file);
}

/// `(session, path, path length) -> file`: the file at `path`, read once per
/// session, or -1 when it cannot be read.
fn read(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, address, length] = arguments else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let Some(path) = read_text(memory, *address, *length) else {
        return MISSING;
    };
    state
        .read_source(Path::new(&path))
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// `(session, path, path length) -> 1 or 0`: whether a file or directory
/// is at `path`.
fn path_exists(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, address, length] = arguments else {
        return 0;
    };
    if *session != state.session.as_u64() {
        return 0;
    }
    read_text(memory, *address, *length).map_or(0, |path| {
        let path = PathBuf::from(path);
        let found = path_present(&state.overlay, &path);
        state.observations.push(Observation::Exists { path, found });
        u64::from(found)
    })
}

fn virtual_source(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, origin, address, length] = arguments else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let Some(text) = read_text(memory, *address, *length) else {
        return MISSING;
    };
    state
        .add_virtual_source(*origin, text)
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// `(session, file, origin, start, end, name, length)`: `file` holds what the
/// macro `name` invoked over `start..end` of `origin` expanded to
/// (`KLF-EDITOR.md` section 13.6). The Session learns it when the phase
/// returns, with the files the phase read.
fn expansion(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, file, origin, start, end, address, length] = arguments else {
        return 0;
    };
    if *session != state.session.as_u64() {
        return 0;
    }
    let (Ok(file), Ok(origin), Ok(start), Ok(end)) = (
        u32::try_from(*file),
        u32::try_from(*origin),
        u32::try_from(*start),
        u32::try_from(*end),
    ) else {
        return 0;
    };
    let Some(name) = read_text(memory, *address, *length) else {
        return 0;
    };
    state.expansions.push(klf_services::Expansion {
        file: FileId::from_u32(file),
        origin: klf_source::Span::new(FileId::from_u32(origin), start, end),
        name,
    });
    0
}

/// `(session, origin, path, length, library, length, selection, length) ->
/// file`: the `.klffi` declarations of the C header at `path`, generated as
/// `klf ffi gen` does for the session's target and added as a source beside
/// `origin`. `includes`, one directory per line, is where `#include` looks
/// after the including file's own directory. The functions are the library's;
/// `selection`, a comma-separated list, keeps only the functions it names when
/// it is not empty. -1 when the header cannot be read or preprocessed.
fn ffi_header(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [
        session,
        origin,
        path_address,
        path_length,
        library_address,
        library_length,
        selection_address,
        selection_length,
        includes_address,
        includes_length,
        scalar_address,
        scalar_length,
    ] = arguments
    else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let (Some(path), Some(library), Some(selection), Some(includes), Some(scalar)) = (
        read_text(memory, *path_address, *path_length),
        read_text(memory, *library_address, *library_length),
        read_text(memory, *selection_address, *selection_length),
        read_text(memory, *includes_address, *includes_length),
        read_text(memory, *scalar_address, *scalar_length),
    ) else {
        return MISSING;
    };
    let withheld: Vec<String> = scalar
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    let include_dirs: Vec<PathBuf> = includes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    let Some(predefined) = klf_klffi::Predefined::for_triple(state.target.triple()) else {
        return MISSING;
    };
    let environment = klf_klffi::Environment {
        predefined: &predefined,
        include_dirs: &include_dirs,
    };
    let Some(header) = state.read_observed(Path::new(&path)) else {
        return MISSING;
    };
    let Ok((mut module, _remarks)) = klf_klffi::generate_withholding(
        &header,
        Path::new(&path),
        &library,
        environment,
        &withheld,
        &mut |candidate: &Path| state.read_observed(candidate).map(|text| text.to_string()),
    ) else {
        return MISSING;
    };
    let selected: Vec<&str> = selection
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if !selected.is_empty() {
        module.items.retain(|item| match item {
            klf_klffi::FfiItem::Function(function) => selected.contains(&function.name.as_str()),
            _ => true,
        });
    }
    for item in &mut module.items {
        match item {
            klf_klffi::FfiItem::Function(function) => {
                for (_, ty) in &mut function.parameters {
                    kira_spelling(ty);
                }
                kira_spelling(&mut function.result);
            }
            klf_klffi::FfiItem::Struct(structure) => {
                for field in &mut structure.fields {
                    kira_spelling(&mut field.ty);
                }
            }
            klf_klffi::FfiItem::Constant(constant) => kira_spelling(&mut constant.ty),
        }
    }
    state
        .add_virtual_source(*origin, klf_klffi::emit(&module))
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// `(session, origin, directory, length, module, length) -> file`: the Kira
/// declarations of `module`'s contract, every `.ridl` file under `directory`,
/// made as `klf ridl bindgen --lang kira` makes them and added as a source
/// named after the contract (KLF-RIDL section 27.7). -1 when the directory
/// holds no `.ridl` file or one cannot be read or parsed.
fn ridl_module(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [
        session,
        _origin,
        directory_address,
        directory_length,
        module_address,
        module_length,
    ] = arguments
    else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let (Some(directory), Some(name)) = (
        read_text(memory, *directory_address, *directory_length),
        read_text(memory, *module_address, *module_length),
    ) else {
        return MISSING;
    };
    let Some(paths) = state.list_observed(Path::new(&directory), "ridl") else {
        return MISSING;
    };
    if paths.is_empty() {
        return MISSING;
    }
    let mut module = klf_ridl::Module::new(name.as_str());
    for path in &paths {
        let Some(text) = state.read_observed(path) else {
            return MISSING;
        };
        let Ok(part) = klf_ridl::parse(FileId::from_u32(0), &name, &text) else {
            return MISSING;
        };
        for imported in part.imports {
            if !module.imports.contains(&imported) {
                module.imports.push(imported);
            }
        }
        module.items.extend(part.items);
    }
    let contract = Path::new(&directory).join(format!("{name}.ridl"));
    state
        .add_named_source(
            format!("{}#kira", contract.display()),
            klf_ridl::bindgen::kira_bindings(&module),
        )
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// Rewrites a header's RIDL scalar to the type Kira spells it as. Kira has no
/// `I64`, `ISize` or `USize`: `Int` is the signed 64-bit integer, and `U64` the
/// unsigned one.
fn kira_spelling(ty: &mut klf_klffi::FfiType) {
    let spelled = match ty.as_str() {
        "I64" | "ISize" => "Int",
        "USize" => "U64",
        _ => return,
    };
    spelled.clone_into(ty);
}

/// `(session, directory, length, extension, length) -> count`: reads every
/// file under `directory` whose extension is `extension`, in path order, and
/// keeps the list for `directory_file`. -1 when the directory cannot be read.
fn directory(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [
        session,
        address,
        length,
        extension_address,
        extension_length,
    ] = arguments
    else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    let (Some(directory), Some(extension)) = (
        read_text(memory, *address, *length),
        read_text(memory, *extension_address, *extension_length),
    ) else {
        return MISSING;
    };
    let Some(paths) = state.list_observed(Path::new(&directory), &extension) else {
        return MISSING;
    };
    let mut listing = Vec::with_capacity(paths.len());
    for path in &paths {
        match state.read_source(path) {
            Some(file) => listing.push(file),
            None => return MISSING,
        }
    }
    state.listing = listing;
    u64::try_from(state.listing.len()).unwrap_or(MISSING)
}

/// `(session, index) -> file`: one entry of the last `directory` listing.
fn directory_file(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    let [session, index] = arguments else {
        return MISSING;
    };
    if *session != state.session.as_u64() {
        return MISSING;
    }
    usize::try_from(*index)
        .ok()
        .and_then(|index| state.listing.get(index))
        .map_or(MISSING, |file| u64::from(file.as_u32()))
}

/// Whether a file or directory is at `path`, or a document stands in for one.
#[must_use]
pub fn path_present<S: std::hash::BuildHasher>(
    overlay: &std::collections::HashMap<PathBuf, Arc<str>, S>,
    path: &Path,
) -> bool {
    klf_services::note_access(path);
    path.exists() || overlay.contains_key(&klf_services::overlay_key(path))
}

/// The files under `directory` with `extension`, in path order, as a phase
/// and a later check of what it observed both list them.
#[must_use]
pub fn listing(directory: &Path, extension: &str) -> Option<Vec<PathBuf>> {
    klf_services::note_access(directory);
    let mut paths = Vec::new();
    collect(directory, extension, &mut paths).ok()?;
    paths.sort();
    Some(paths)
}

fn collect(directory: &Path, extension: &str, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, extension, paths)?;
        } else if path.extension().is_some_and(|found| found == extension) {
            paths.push(path);
        }
    }
    Ok(())
}

fn read_text(memory: &mut dyn ServiceMemory, address: u64, length: u64) -> Option<String> {
    let length = usize::try_from(length).ok()?;
    if length > MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if !memory.read_buffer(address, &mut bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

impl DefinitionServiceState {
    /// The text at `path` through the overlay, noted as observed.
    fn read_observed(&mut self, path: &Path) -> Option<Arc<str>> {
        let text = klf_services::read_through(&self.overlay, path).ok();
        self.observations.push(Observation::Read {
            path: path.to_path_buf(),
            text: text.clone(),
        });
        text
    }

    /// The files under `directory` with `extension`, in path order, noted as
    /// observed.
    fn list_observed(&mut self, directory: &Path, extension: &str) -> Option<Vec<PathBuf>> {
        let paths = listing(directory, extension);
        self.observations.push(Observation::Listing {
            directory: directory.to_path_buf(),
            extension: extension.to_owned(),
            paths: paths.clone(),
        });
        paths
    }

    pub(super) fn add_virtual_source(&mut self, origin: u64, text: String) -> Option<FileId> {
        let origin = usize::try_from(origin).ok()?;
        let (_, name, _) = self.sources.get(origin)?;
        let file = FileId::from_u32(u32::try_from(self.sources.len()).ok()?);
        let name = format!("{name}#generated{}", file.as_u32());
        self.source_hashes.push(klf_source::source_hash(&text));
        self.sources.push((file, name, Arc::from(text)));
        self.read_count += 1;
        Some(file)
    }

    /// A source the session made rather than read, under `name`, read once:
    /// a second request for the same name is the file made the first time.
    pub(super) fn add_named_source(&mut self, name: String, text: String) -> Option<FileId> {
        if let Some((file, _, _)) = self.sources.iter().find(|(_, known, _)| *known == name) {
            return Some(*file);
        }
        let file = FileId::from_u32(u32::try_from(self.sources.len()).ok()?);
        self.source_hashes.push(klf_source::source_hash(&text));
        self.sources.push((file, name, Arc::from(text)));
        self.read_count += 1;
        Some(file)
    }

    /// The file at `path`: the one already in the session when its name or
    /// canonical path matches, or a new one read from disk.
    fn read_source(&mut self, path: &Path) -> Option<FileId> {
        // Each source's canonical path is asked of the file system once.
        for (file, name, _) in &self.sources[self.paths_indexed..] {
            self.source_paths
                .entry(Path::new(name).to_path_buf())
                .or_insert(*file);
            if let Ok(canonical) = std::fs::canonicalize(name) {
                self.source_paths.entry(canonical).or_insert(*file);
            }
        }
        self.paths_indexed = self.sources.len();
        let known = self
            .source_paths
            .get(path)
            .or_else(|| self.source_paths.get(&klf_services::overlay_key(path)))
            .copied();
        if let Some(file) = known {
            let text = self
                .sources
                .iter()
                .find(|(known, _, _)| *known == file)
                .map(|(_, _, text)| Arc::clone(text));
            self.observations.push(Observation::Read {
                path: path.to_path_buf(),
                text,
            });
            return Some(file);
        }
        let text = self.read_observed(path)?;
        let file = FileId::from_u32(u32::try_from(self.sources.len()).ok()?);
        self.source_hashes.push(klf_source::source_hash(&text));
        self.sources
            .push((file, path.to_string_lossy().into_owned(), text));
        self.read_count += 1;
        Some(file)
    }

    /// Registers the files this phase read in `session`, in the order they were
    /// read, which gives each the `FileId` the phase already handed out.
    pub(crate) fn commit_records(&mut self, session: &mut Session) -> Result<(), String> {
        if let Some(facts) = self.facts.take() {
            session.record_facts(facts);
        }
        session.observe(std::mem::take(&mut self.observations));
        let first = self.sources.len() - self.read_count;
        for (file, name, text) in &self.sources[first..] {
            let registered = session.add_file(name.as_str(), text.clone());
            if registered != *file {
                return Err(format!("the session gave `{name}` a different file id"));
            }
        }
        self.read_count = 0;
        for expansion in std::mem::take(&mut self.expansions) {
            session.record_expansion(expansion);
        }
        Ok(())
    }
}
