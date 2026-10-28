use crate::{DefinitionServiceState, ServiceSink};
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory, copy_out};
use std::sync::{Arc, Mutex};

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state("klfsvc_source_file_name_len", state, file_name_len);
    services.register_state("klfsvc_source_file_name_byte", state, file_name_byte);
    services.register_state_with_memory("klfsvc_source_file_name_copy", state, file_name_copy);
    services.register_state("klfsvc_source_name_len", state, current_name_len);
    services.register_state("klfsvc_source_name_byte", state, current_name_byte);
    services.register_state_with_memory("klfsvc_source_name_copy", state, current_name_copy);
    services.register_state_with_memory("klfsvc_source_named_exists", state, named_exists);
    services.register_state_with_memory("klfsvc_source_named_len", state, named_len);
    services.register_state_with_memory("klfsvc_source_named_byte", state, named_byte);
    services.register_state_with_memory("klfsvc_source_named_copy", state, named_copy);
    crate::file_services::register(state, services);
    crate::subquery_services::register(state, services);
}

fn current_name_len(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    current_name(state, arguments).map_or(u64::MAX, |name| {
        u64::try_from(name.len()).unwrap_or(u64::MAX)
    })
}

fn current_name_byte(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    let Some(name) = current_name(state, &[]) else {
        return u64::MAX;
    };
    let Some(index) = arguments
        .first()
        .copied()
        .and_then(|index| usize::try_from(index).ok())
    else {
        return 0;
    };
    name.as_bytes().get(index).copied().map_or(0, u64::from)
}

fn current_name_copy(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let (Some(name), [address, length]) = (current_name(state, &[]), arguments) else {
        return 0;
    };
    copy_out(memory, name.as_bytes(), *address, *length)
}

fn current_name<'a>(state: &'a DefinitionServiceState, arguments: &[u64]) -> Option<&'a str> {
    if !arguments.is_empty() {
        return None;
    }
    let file = state.file?;
    state
        .sources
        .iter()
        .find(|(known, _, _)| *known == file)
        .map(|(_, name, _)| name.as_str())
}

fn file_name_len(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    file_name(state, arguments).map_or(u64::MAX, |name| {
        u64::try_from(name.len()).unwrap_or(u64::MAX)
    })
}

fn file_name_byte(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    let Some(name) = file_name(state, arguments) else {
        return u64::MAX;
    };
    let Some(index) = arguments
        .get(2)
        .copied()
        .and_then(|index| usize::try_from(index).ok())
    else {
        return 0;
    };
    name.as_bytes().get(index).copied().map_or(0, u64::from)
}

fn file_name_copy(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let (Some(name), [_, _, address, length]) = (file_name(state, arguments), arguments) else {
        return 0;
    };
    copy_out(memory, name.as_bytes(), *address, *length)
}

fn file_name<'a>(state: &'a DefinitionServiceState, arguments: &[u64]) -> Option<&'a str> {
    let [session, file, ..] = arguments else {
        return None;
    };
    if *session != state.session.as_u64() {
        return None;
    }
    state
        .sources
        .iter()
        .find(|(known, _, _)| u64::from(known.as_u32()) == *file)
        .map(|(_, name, _)| name.as_str())
}

fn named_exists(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    read_name(memory, arguments)
        .and_then(|name| named_observed(state, &name))
        .map_or(0, |_| 1)
}

fn named_len(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    read_name(memory, arguments)
        .and_then(|name| named_observed(state, &name))
        .map_or(0, |text| u64::try_from(text.len()).unwrap_or(u64::MAX))
}

/// The text of the source named `name`, noted as observed: a header the
/// session registered for an `#include` is no file of the unit, so a cached
/// answer holds only while the header still reads the same. A name that is
/// neither a source nor on disk is noted as absent.
fn named_observed(state: &mut DefinitionServiceState, name: &str) -> Option<Arc<str>> {
    let found = named_entry(state, name)
        .map(|(path, text)| (std::path::PathBuf::from(path), Arc::clone(text)));
    let observation = if let Some((path, text)) = &found {
        klf_services::Observation::Read {
            path: path.clone(),
            text: Some(Arc::clone(text)),
        }
    } else {
        let path = std::path::PathBuf::from(name);
        if !path.is_absolute() || path.exists() {
            return None;
        }
        klf_services::Observation::Exists { path, found: false }
    };
    if !state.observations.contains(&observation) {
        state.observations.push(observation);
    }
    found.map(|(_, text)| text)
}

fn named_byte(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [_, _, index] = arguments else {
        return 0;
    };
    let Some(name) = read_name(memory, arguments) else {
        return 0;
    };
    let Some(text) = named_source(state, &name) else {
        return 0;
    };
    let Some(index) = usize::try_from(*index).ok() else {
        return 0;
    };
    text.as_bytes().get(index).copied().map_or(0, u64::from)
}

fn named_copy(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [_, _, address, length] = arguments else {
        return 0;
    };
    let Some(name) = read_name(memory, arguments) else {
        return 0;
    };
    let Some(text) = named_source(state, &name) else {
        return 0;
    };
    copy_out(memory, text.as_bytes(), *address, *length)
}

fn named_source<'a>(state: &'a DefinitionServiceState, name: &str) -> Option<&'a str> {
    named_entry(state, name).map(|(_, text)| text.as_ref())
}

/// The name and text of the source named `name`, or ending `/name`.
fn named_entry<'a>(
    state: &'a DefinitionServiceState,
    name: &str,
) -> Option<(&'a str, &'a Arc<str>)> {
    if let Some((_, known, text)) = state.sources.iter().find(|(_, known, _)| known == name) {
        return Some((known.as_str(), text));
    }
    let suffix = format!("/{name}");
    state
        .sources
        .iter()
        .find(|(_, known, _)| known.ends_with(&suffix))
        .map(|(_, known, text)| (known.as_str(), text))
}

fn read_name(memory: &mut dyn ServiceMemory, arguments: &[u64]) -> Option<String> {
    let [address, length, ..] = arguments else {
        return None;
    };
    let length = usize::try_from(*length).ok()?;
    if length > MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if !memory.read_buffer(*address, &mut bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
