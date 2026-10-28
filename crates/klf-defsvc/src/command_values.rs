//! The `klf.cli.*` services one call of a definition's `__klf_command` entry
//! reads its parsed arguments through (`KLF-CLI.md` section 4).

use crate::ServiceSink;
use klf_args::Value;
use klf_rt::MAX_SERVICE_BUFFER_BYTES;
use std::sync::Arc;

/// The KIR entry the KLF Kira lowerer generates for a package's
/// `ParsableCommand`s. It takes the command's index and returns the exit
/// status its `run` returns, or -1 for an index no command has.
pub const COMMAND_ENTRY: &str = "__klf_command";

/// Serves `values`, one list per argument slot of the selected command, as
/// [`klf_args::Matches::values`] holds them.
pub fn register_command_values(values: Vec<Vec<Value>>, services: &mut ServiceSink) {
    let values = Arc::new(values);
    let counts = Arc::clone(&values);
    services.register("klfsvc_cli_count", move |words| {
        let [index] = words else { return 0 };
        slot(&counts, *index).map_or(0, |slot| slot.len() as u64)
    });
    let integers = Arc::clone(&values);
    services.register("klfsvc_cli_int", move |words| {
        let [index, at] = words else { return 0 };
        let value =
            slot(&integers, *index).and_then(|slot| slot.get(usize::try_from(*at).ok()?).cloned());
        match value {
            Some(Value::Bool(value)) => u64::from(value),
            Some(Value::Int(value)) => value.cast_unsigned(),
            Some(Value::Choice(index)) => u64::from(index),
            _ => 0,
        }
    });
    let lengths = Arc::clone(&values);
    services.register("klfsvc_cli_string_len", move |words| {
        let [index, at] = words else { return u64::MAX };
        text(&lengths, *index, *at)
            .filter(|text| text.len() <= MAX_SERVICE_BUFFER_BYTES)
            .map_or(u64::MAX, |text| text.len() as u64)
    });
    services.register_with_memory("klfsvc_cli_string_copy", move |words, memory| {
        let [index, at, address, length] = words else {
            return 0;
        };
        let Some(text) = text(&values, *index, *at) else {
            return 0;
        };
        if usize::try_from(*length).ok() != Some(text.len()) {
            return 0;
        }
        u64::from(memory.write_buffer(*address, text.as_bytes()))
    });
}

fn slot(values: &[Vec<Value>], word: u64) -> Option<&[Value]> {
    values.get(usize::try_from(word).ok()?).map(Vec::as_slice)
}

fn text(values: &[Vec<Value>], index: u64, at: u64) -> Option<String> {
    match slot(values, index)?.get(usize::try_from(at).ok()?)? {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}
