//! The reader and writer services an out calls (`KLF-KSL.md` section 6.7),
//! over the one unit the driver is emitting. The unit word is not checked:
//! an out runs over one unit at a time, so there is no other it could name.

use crate::ServiceSink;
use klf_rt::ServiceMemory;
use klf_shader_out::UnitView;
use std::sync::{Arc, Mutex};

const NONE: u64 = u64::MAX;

fn text(memory: &dyn ServiceMemory, address: u64, length: u64) -> Option<String> {
    let length = usize::try_from(length).ok()?;
    if length > klf_rt::MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if length > 0 && !memory.read_buffer(address, &mut bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn signed(word: u64) -> i64 {
    word.cast_signed()
}

fn answer(value: i64) -> u64 {
    value.cast_unsigned()
}

/// Registers the services over `view`.
pub fn register_reader(sink: &mut ServiceSink, view: &Arc<Mutex<UnitView>>) {
    register_reads(sink, view);
    register_out(sink, view);
}

/// Registers the services a shader reads the unit's tables through.
fn register_reads(sink: &mut ServiceSink, view: &Arc<Mutex<UnitView>>) {
    sink.register_state_with_memory("klfsvc_shader_read_count", view, |view, words, memory| {
        let [_, table, length, owner] = words else {
            return NONE;
        };
        text(memory, *table, *length)
            .map_or(NONE, |table| answer(view.count(&table, signed(*owner))))
    });
    sink.register_state_with_memory("klfsvc_shader_read_kind", view, |view, words, memory| {
        let [_, table, length, owner, index] = words else {
            return NONE;
        };
        text(memory, *table, *length).map_or(NONE, |table| {
            answer(view.kind(&table, signed(*owner), signed(*index)))
        })
    });
    for (symbol, integer) in [
        ("klfsvc_shader_read_int", true),
        ("klfsvc_shader_read_text", false),
    ] {
        sink.register_state_with_memory(symbol, view, move |view, words, memory| {
            let [
                _,
                table,
                table_length,
                owner,
                index,
                field,
                field_length,
                item,
            ] = words
            else {
                return NONE;
            };
            let (Some(table), Some(field)) = (
                text(memory, *table, *table_length),
                text(memory, *field, *field_length),
            ) else {
                return NONE;
            };
            let (owner, index, item) = (signed(*owner), signed(*index), signed(*item));
            answer(if integer {
                view.int(&table, owner, index, &field, item)
            } else {
                view.text(&table, owner, index, &field, item)
            })
        });
    }
    sink.register_state("klfsvc_shader_read_text_len", view, |view, words| {
        let [_, id] = words else {
            return NONE;
        };
        view.text_of(signed(*id))
            .map_or(0, |text| text.len() as u64)
    });
    sink.register_state_with_memory(
        "klfsvc_shader_read_text_copy",
        view,
        |view, words, memory| {
            let [_, id, address, length] = words else {
                return NONE;
            };
            match view.text_of(signed(*id)) {
                Some(text) if text.len() as u64 == *length => {
                    u64::from(text.is_empty() || memory.write_buffer(*address, text.as_bytes()))
                }
                // The misuse is kept already; reading it as empty lets the
                // out run on to its end, where the misuse is reported.
                None => u64::from(*length == 0),
                Some(_) => 0,
            }
        },
    );
}

/// Registers the services a shader's output goes through: its text, and
/// the refusals it reports.
fn register_out(sink: &mut ServiceSink, view: &Arc<Mutex<UnitView>>) {
    sink.register_state_with_memory("klfsvc_shader_out_write", view, |view, words, memory| {
        let [_, address, length] = words else {
            return NONE;
        };
        if let Some(text) = text(memory, *address, *length) {
            view.write(&text);
        }
        0
    });
    sink.register_state_with_memory("klfsvc_shader_out_refuse", view, |view, words, memory| {
        let [_, address, length, function, expression] = words else {
            return NONE;
        };
        if let Some(message) = text(memory, *address, *length) {
            view.refuse(&message, signed(*function), signed(*expression));
        }
        0
    });
    sink.register_state_with_memory(
        "klfsvc_shader_out_refuse_at",
        view,
        |view, words, memory| {
            let [_, address, length, table, table_length, owner, index] = words else {
                return NONE;
            };
            if let (Some(message), Some(table)) = (
                text(memory, *address, *length),
                text(memory, *table, *table_length),
            ) {
                view.refuse_at(&message, &table, signed(*owner), signed(*index));
            }
            0
        },
    );
}
