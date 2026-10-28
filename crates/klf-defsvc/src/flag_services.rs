use crate::{DefinitionServiceState, ServiceSink};
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory};
use klf_services::DriverFlagValue;
use std::sync::{Arc, Mutex};

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state_with_memory(
        "klfsvc_session_flag",
        state,
        |state, arguments, memory| lookup(state, arguments, memory),
    );
    services.register_state_with_memory(
        "klfsvc_session_flag_string_copy",
        state,
        |state, arguments, memory| copy_string(state, arguments, memory),
    );
    services.register_state("klfsvc_session_std_root_len", state, std_root_len);
    services.register_state_with_memory("klfsvc_session_std_root_copy", state, std_root_copy);
    services.register_state("klfsvc_session_target_len", state, target_len);
    services.register_state_with_memory("klfsvc_session_target_copy", state, target_copy);
}

fn target_len(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    match arguments {
        [session] if *session == state.session.as_u64() => {
            u64::try_from(state.target.triple().len()).unwrap_or(u64::MAX)
        }
        _ => u64::MAX,
    }
}

fn target_copy(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    match arguments {
        [session, address, length] if *session == state.session.as_u64() => {
            klf_rt::copy_out(memory, state.target.triple().as_bytes(), *address, *length)
        }
        _ => 0,
    }
}

fn std_root_len(state: &mut DefinitionServiceState, arguments: &[u64]) -> u64 {
    match arguments {
        [session] if *session == state.session.as_u64() => {
            u64::try_from(state.std_root.len()).unwrap_or(u64::MAX)
        }
        _ => u64::MAX,
    }
}

fn std_root_copy(
    state: &mut DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    match arguments {
        [session, address, length] if *session == state.session.as_u64() => {
            klf_rt::copy_out(memory, state.std_root.as_bytes(), *address, *length)
        }
        _ => 0,
    }
}

fn lookup(
    state: &DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [session, name_address, name_length, output_address] = arguments else {
        return u64::MAX;
    };
    if *session != state.session.as_u64() {
        return u64::MAX;
    }
    let Some(name) = read_text(memory, *name_address, *name_length) else {
        return u64::MAX;
    };
    let Some(value) = state.driver_flags.get(&name) else {
        return write_lookup(memory, *output_address, u64::MAX, 0);
    };
    let (tag, payload) = match value {
        DriverFlagValue::Boolean(value) => (0, u64::from(*value)),
        DriverFlagValue::Integer(value) => (1, (*value).cast_unsigned()),
        DriverFlagValue::String(value) => {
            if value.len() > MAX_SERVICE_BUFFER_BYTES {
                return write_lookup(memory, *output_address, u64::MAX, 0);
            }
            (2, u64::try_from(value.len()).unwrap_or(u64::MAX))
        }
    };
    write_lookup(memory, *output_address, tag, payload)
}

fn copy_string(
    state: &DefinitionServiceState,
    arguments: &[u64],
    memory: &mut dyn ServiceMemory,
) -> u64 {
    let [
        session,
        name_address,
        name_length,
        output_address,
        output_length,
    ] = arguments
    else {
        return 0;
    };
    if *session != state.session.as_u64() {
        return 0;
    }
    let Some(name) = read_text(memory, *name_address, *name_length) else {
        return 0;
    };
    let Some(DriverFlagValue::String(value)) = state.driver_flags.get(&name) else {
        return 0;
    };
    if usize::try_from(*output_length).ok() != Some(value.len())
        || !memory.write_buffer(*output_address, value.as_bytes())
    {
        return 0;
    }
    1
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

fn write_lookup(memory: &mut dyn ServiceMemory, address: u64, tag: u64, payload: u64) -> u64 {
    let mut result = [0; 16];
    result[..8].copy_from_slice(&tag.to_le_bytes());
    result[8..].copy_from_slice(&payload.to_le_bytes());
    u64::from(memory.write_buffer(address, &result))
}
