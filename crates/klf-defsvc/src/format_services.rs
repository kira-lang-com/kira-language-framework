//! `klf.format.*` (`KLF-FORMAT.md` section 4): the services a `format` hook
//! describes its layout through. Outside a `format` call there is no layout
//! and every service answers 0 without reading its arguments.

use crate::{DefinitionServiceBridge, DefinitionServiceState, ServiceSink};
use klf_format::{Builder, LineKind, Syntax};
use klf_rt::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory};
use std::sync::{Arc, Mutex};

type TakeSpan = fn(&mut Builder, usize, usize) -> bool;
type Begin = fn(&mut Builder);

pub(super) fn register(state: &Arc<Mutex<DefinitionServiceState>>, services: &mut ServiceSink) {
    services.register_state_with_memory("klfsvc_format_text", state, |state, words, memory| {
        let Some(builder) = state.format.as_mut() else {
            return 0;
        };
        let [address, length] = words else {
            return 0;
        };
        let text = read_text(memory, *address, *length);
        u64::from(text.is_some_and(|text| builder.text(&text)))
    });
    let spans: [(&str, TakeSpan); 2] = [
        ("klfsvc_format_token", Builder::token),
        ("klfsvc_format_verbatim", Builder::verbatim),
    ];
    for (name, take) in spans {
        services.register_state(name, state, move |state, words| {
            let (Some(builder), [start, end]) = (state.format.as_mut(), words) else {
                return 0;
            };
            let (Ok(start), Ok(end)) = (usize::try_from(*start), usize::try_from(*end)) else {
                return 0;
            };
            u64::from(take(builder, start, end))
        });
    }
    services.register_state("klfsvc_format_line", state, |state, words| {
        let (Some(builder), Some(kind)) = (
            state.format.as_mut(),
            words.first().copied().and_then(LineKind::from_code),
        ) else {
            return 0;
        };
        builder.line(kind);
        1
    });
    let frames: [(&str, Begin); 2] = [
        ("klfsvc_format_group", Builder::begin_group),
        ("klfsvc_format_indent", Builder::begin_indent),
    ];
    for (name, begin) in frames {
        services.register_state(name, state, move |state, _| {
            state.format.as_mut().map_or(0, |builder| {
                begin(builder);
                1
            })
        });
    }
    services.register_state("klfsvc_format_end", state, |state, _| {
        state
            .format
            .as_mut()
            .map_or(0, |builder| u64::from(builder.end()))
    });
}

fn read_text(memory: &mut dyn ServiceMemory, address: u64, length: u64) -> Option<String> {
    let length = usize::try_from(length).ok()?;
    if length > MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    (length == 0 || memory.read_buffer(address, &mut bytes)).then_some(())?;
    String::from_utf8(bytes).ok()
}

impl DefinitionServiceBridge {
    /// Opens a layout over the phase's file, for a `format` hook call.
    ///
    /// # Errors
    /// Returns a message when the bridge lock is poisoned.
    pub fn begin_format(&self, syntax: Syntax) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "definition service bridge lock poisoned".to_owned())?;
        state.format = Some(Builder::new(state.source.clone(), syntax));
        Ok(())
    }

    /// The layout the `format` hook described.
    ///
    /// # Errors
    /// Returns a message when the bridge lock is poisoned or no layout was
    /// opened.
    pub fn take_format(&self) -> Result<Builder, String> {
        self.state
            .lock()
            .map_err(|_| "definition service bridge lock poisoned".to_owned())?
            .format
            .take()
            .ok_or_else(|| "no format layout was opened".to_owned())
    }
}
