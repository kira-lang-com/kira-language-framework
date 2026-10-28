//! The standard kernel service / host-capability sets (section 15.7, 8.12).
//! These are the real kernel-provided functions a running program reaches by
//! name. The `klf.io` set writes output, and the source set exposes the current
//! input buffer to a running definition. Both are pure host services — no
//! `unsafe`, no dynamic symbol lookup.

use crate::capabilities::Capabilities;
use klf_diag::RuntimeDiagnostic;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// The VM capabilities, output sink and retained runtime diagnostics for one run.
pub type KernelServicesWithDiagnostics = (
    Capabilities,
    Arc<Mutex<Vec<u8>>>,
    Arc<Mutex<Vec<RuntimeDiagnostic>>>,
);

/// The default kernel services, writing `klf.io` output to a byte sink. The sink
/// is returned so an embedder (or a test) can read what the program wrote; a
/// real runner would pass one backed by the process's stdout.
#[must_use]
pub fn kernel_services() -> (Capabilities, Arc<Mutex<Vec<u8>>>) {
    kernel_services_with_source([])
}

/// The default services with a source buffer available to definition code.
#[must_use]
pub fn kernel_services_with_source(
    source: impl AsRef<[u8]>,
) -> (Capabilities, Arc<Mutex<Vec<u8>>>) {
    let (capabilities, sink, _) = kernel_services_with_source_and_diagnostics(source);
    (capabilities, sink)
}

/// The default services with source text and typed runtime diagnostics.
#[must_use]
pub fn kernel_services_with_source_and_diagnostics(
    source: impl AsRef<[u8]>,
) -> KernelServicesWithDiagnostics {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let mut capabilities = Capabilities::new();
    let source = Arc::new(source.as_ref().to_vec());

    register_io_services(&mut capabilities, &sink);

    let source_length = Arc::clone(&source);
    capabilities.register("klfsvc_source_len", move |_| {
        u64::try_from(source_length.len()).unwrap_or(u64::MAX)
    });

    let source_bytes = Arc::clone(&source);
    capabilities.register("klfsvc_source_byte", move |arguments| {
        let Some(index) = arguments
            .first()
            .and_then(|value| i64::try_from(*value).ok())
        else {
            return 0;
        };
        let Ok(index) = usize::try_from(index) else {
            return 0;
        };
        source_bytes.get(index).copied().map_or(0, u64::from)
    });

    let source_copy = Arc::clone(&source);
    capabilities.register_with_memory("klfsvc_source_copy", move |arguments, memory| {
        let [address, length] = arguments else {
            return 0;
        };
        klf_rt::copy_out(memory, &source_copy, *address, *length)
    });

    register_diagnostic_services(&mut capabilities, &sink, &diagnostics);

    register_text_services(&mut capabilities);
    register_math_services(&mut capabilities);
    register_number_services(&mut capabilities);
    crate::file_services::register(&mut capabilities);
    crate::child_services::register(&mut capabilities);
    crate::net_services::register(&mut capabilities);
    crate::crypto_services::register(&mut capabilities);
    crate::thread_services::register(&mut capabilities);
    register_process_services(&mut capabilities);
    register_time_services(&mut capabilities);
    register_host_services(&mut capabilities);
    crate::trap_operands::register(&mut capabilities);

    // Every symbol in the service manifest must resolve, so a program that
    // calls a service this host does not implement gets the error sentinel
    // rather than a missing capability. Real handlers registered above win.
    for entry in include_str!("../../../kernel/services.manifest").lines() {
        let entry = entry.split('#').next().unwrap_or_default();
        let Some(symbol) = entry.split_whitespace().nth(1) else {
            continue;
        };
        if !capabilities.contains(symbol) {
            capabilities.register(symbol, |_| u64::MAX);
        }
    }

    (capabilities, sink, diagnostics)
}

/// A service argument read as a signed word, saturating rather than wrapping.
fn word(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Records one reported diagnostic and echoes it on the service sink, in the
/// shape the native runtime prints so both paths parse the same.
/// `klf.io`: bytes and integers to the run's output, and bytes to standard
/// error.
fn register_io_services(capabilities: &mut Capabilities, sink: &Arc<Mutex<Vec<u8>>>) {
    let byte_sink = Arc::clone(sink);
    capabilities.register("klfsvc_io_put", move |arguments| {
        let byte = arguments.first().copied().unwrap_or(0);
        #[allow(clippy::cast_possible_truncation)]
        let byte = byte as u8;
        if let Ok(mut buffer) = byte_sink.lock() {
            let _ = buffer.write_all(&[byte]);
        }
        u64::from(byte)
    });

    let write_sink = Arc::clone(sink);
    capabilities.register_with_memory("klfsvc_io_write", move |arguments, memory| {
        let [address, length] = arguments else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*length)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if count > 0 && !memory.read_buffer(*address, &mut bytes) {
            return u64::MAX;
        }
        if let Ok(mut buffer) = write_sink.lock() {
            if buffer.write_all(&bytes).is_err() {
                return u64::MAX;
            }
        } else {
            return u64::MAX;
        }
        *length
    });

    capabilities.register_with_memory("klfsvc_io_write_error", |arguments, memory| {
        let [address, length] = arguments else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*length)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if count > 0 && !memory.read_buffer(*address, &mut bytes) {
            return u64::MAX;
        }
        let mut errors = std::io::stderr().lock();
        if errors
            .write_all(&bytes)
            .and_then(|()| errors.flush())
            .is_err()
        {
            return u64::MAX;
        }
        *length
    });

    let int_sink = Arc::clone(sink);
    capabilities.register("klfsvc_io_puti", move |arguments| {
        #[allow(clippy::cast_possible_wrap)]
        let value = arguments.first().copied().unwrap_or(0) as i64;
        if let Ok(mut buffer) = int_sink.lock() {
            let _ = writeln!(buffer, "{value}");
        }
        0
    });
}

/// `klf.diag`: diagnostics recorded for the driver, and holds that keep back
/// the ones reported inside them.
fn register_diagnostic_services(
    capabilities: &mut Capabilities,
    sink: &Arc<Mutex<Vec<u8>>>,
    diagnostics: &Arc<Mutex<Vec<RuntimeDiagnostic>>>,
) {
    let holds = Arc::new(Mutex::new(Vec::<(usize, usize)>::new()));
    let hold_points = Arc::clone(&holds);
    let hold_records = Arc::clone(diagnostics);
    let hold_sink = Arc::clone(sink);
    capabilities.register("klfsvc_diag_hold", move |_| {
        match (hold_points.lock(), hold_records.lock(), hold_sink.lock()) {
            (Ok(mut points), Ok(records), Ok(bytes)) => points.push((records.len(), bytes.len())),
            _ => return u64::MAX,
        }
        0
    });
    let release_records = Arc::clone(diagnostics);
    let release_sink = Arc::clone(sink);
    capabilities.register("klfsvc_diag_release", move |arguments| {
        let Ok(mut points) = holds.lock() else {
            return u64::MAX;
        };
        let Some((start, byte_start)) = points.pop() else {
            return 0;
        };
        let Ok(mut records) = release_records.lock() else {
            return u64::MAX;
        };
        let count = records.len().saturating_sub(start);
        if arguments.first().copied().unwrap_or(0) == 0 {
            records.truncate(start);
            let Ok(mut bytes) = release_sink.lock() else {
                return u64::MAX;
            };
            bytes.truncate(byte_start);
        }
        u64::try_from(count).unwrap_or(u64::MAX)
    });

    let diagnostic_sink = Arc::clone(sink);
    let diagnostic_records = Arc::clone(diagnostics);
    capabilities.register("klfsvc_diag_error", move |arguments| {
        let code = arguments.first().copied().unwrap_or(0);
        let start = arguments.get(1).copied().unwrap_or(0);
        let end = arguments.get(2).copied().unwrap_or(start);
        report_diagnostic(
            &diagnostic_records,
            &diagnostic_sink,
            RuntimeDiagnostic::new(word(code), word(start), word(end)),
        )
    });

    let located_sink = Arc::clone(sink);
    let located_records = Arc::clone(diagnostics);
    capabilities.register("klfsvc_diag_error_in", move |arguments| {
        let file = arguments.first().copied().unwrap_or(0);
        let code = arguments.get(1).copied().unwrap_or(0);
        let start = arguments.get(2).copied().unwrap_or(0);
        let end = arguments.get(3).copied().unwrap_or(start);
        report_diagnostic(
            &located_records,
            &located_sink,
            RuntimeDiagnostic {
                file: word(file),
                ..RuntimeDiagnostic::new(word(code), word(start), word(end))
            },
        )
    });

    let subject_sink = Arc::clone(sink);
    let subject_records = Arc::clone(diagnostics);
    capabilities.register("klfsvc_diag_error_subject", move |arguments| {
        let file = arguments.first().copied().unwrap_or(0);
        let code = arguments.get(1).copied().unwrap_or(0);
        let start = arguments.get(2).copied().unwrap_or(0);
        let end = arguments.get(3).copied().unwrap_or(start);
        let subject_start = arguments.get(4).copied().unwrap_or(start);
        let subject_end = arguments.get(5).copied().unwrap_or(subject_start);
        report_diagnostic(
            &subject_records,
            &subject_sink,
            RuntimeDiagnostic {
                file: word(file),
                subject: Some((word(subject_start), word(subject_end))),
                ..RuntimeDiagnostic::new(word(code), word(start), word(end))
            },
        )
    });

    let detail_sink = Arc::clone(sink);
    let detail_records = Arc::clone(diagnostics);
    capabilities.register_with_memory("klfsvc_diag_detail", move |arguments, memory| {
        let [kind, file, start, end, address, length] = arguments else {
            return u64::MAX;
        };
        let Some(kind) = klf_diag::DetailKind::from_code(word(*kind)) else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*length)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if count > 0 && !memory.read_buffer(*address, &mut bytes) {
            return u64::MAX;
        }
        let detail = klf_diag::RuntimeDetail {
            kind,
            file: word(*file),
            start: word(*start),
            end: word(*end),
            text: String::from_utf8_lossy(&bytes).into_owned(),
        };
        if let Ok(mut buffer) = detail_sink.lock() {
            let _ = writeln!(buffer, "{}", detail.protocol_line());
        }
        if let Ok(mut records) = detail_records.lock()
            && let Some(last) = records.last_mut()
        {
            last.details.push(detail);
        }
        0
    });

    let origin_sink = Arc::clone(sink);
    let origin_records = Arc::clone(diagnostics);
    capabilities.register("klfsvc_diag_error_origin", move |arguments| {
        let origin = arguments.first().copied().unwrap_or(0);
        let code = arguments.get(1).copied().unwrap_or(0);
        let start = arguments.get(2).copied().unwrap_or(0);
        let end = arguments.get(3).copied().unwrap_or(start);
        report_diagnostic(
            &origin_records,
            &origin_sink,
            RuntimeDiagnostic {
                origin: word(origin),
                ..RuntimeDiagnostic::new(word(code), word(start), word(end))
            },
        )
    });
}

fn report_diagnostic(
    records: &Arc<Mutex<Vec<RuntimeDiagnostic>>>,
    sink: &Arc<Mutex<Vec<u8>>>,
    record: RuntimeDiagnostic,
) -> u64 {
    if let Ok(mut records) = records.lock() {
        records.push(record.clone());
    }
    if let Ok(mut buffer) = sink.lock() {
        match record.subject {
            Some((subject_start, subject_end)) => {
                let _ = writeln!(
                    buffer,
                    "DIAGS {} {} {} {} {} {} {}",
                    record.code, record.start, record.end, record.file, record.origin, subject_start, subject_end
                );
            }
            None => {
                let _ = writeln!(
                    buffer,
                    "DIAG {} {} {} {} {}",
                    record.code, record.start, record.end, record.file, record.origin
                );
            }
        }
    }
    0
}

fn register_math_services(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_math_unary", |arguments| {
        let [operation, bits] = arguments else {
            return f64::NAN.to_bits();
        };
        klf_rt::math::unary(*operation, f64::from_bits(*bits))
            .unwrap_or(f64::NAN)
            .to_bits()
    });
    capabilities.register("klfsvc_math_binary", |arguments| {
        let [operation, left, right] = arguments else {
            return f64::NAN.to_bits();
        };
        klf_rt::math::binary(*operation, f64::from_bits(*left), f64::from_bits(*right))
            .unwrap_or(f64::NAN)
            .to_bits()
    });
}

/// `klfsvc_number`: Kira's exact decimal `Number`, one operation per call. A
/// `Number` is two words and a service answers one, so the first word comes
/// back and `klfsvc_number_second` hands over the second.
fn register_number_services(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_number", |arguments| {
        let [operation, left, left_scale, right, right_scale] = arguments else {
            return klf_rt::decimal::service(u64::MAX, [0, 0], [0, 0]);
        };
        klf_rt::decimal::service(*operation, [*left, *left_scale], [*right, *right_scale])
    });
    capabilities.register("klfsvc_number_second", |_| {
        klf_rt::decimal::service_second()
    });
}

fn register_text_services(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_text_float", |arguments, memory| {
        let [bits, destination, capacity] = arguments else {
            return u64::MAX;
        };
        let text = f64::from_bits(*bits).to_string();
        let bytes = text.as_bytes();
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if usize::try_from(*capacity).is_ok_and(|capacity| bytes.len() <= capacity)
            && !bytes.is_empty()
            && !memory.write_buffer(*destination, bytes)
        {
            return u64::MAX;
        }
        length
    });
}

/// The `klf.process` services: the arguments the hosted program was started
/// with, as its runner recorded them, and the commands it runs.
fn register_time_services(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_time_monotonic", |_| {
        u64::from_ne_bytes(klf_host_process::time::monotonic_nanos().to_ne_bytes())
    });
    capabilities.register("klfsvc_time_wall", |_| {
        u64::from_ne_bytes(klf_host_process::time::wall_nanos().to_ne_bytes())
    });
    capabilities.register("klfsvc_time_sleep", |arguments| {
        let nanos = arguments
            .first()
            .map_or(0, |nanos| i64::from_ne_bytes(nanos.to_ne_bytes()));
        klf_host_process::time::sleep_nanos(nanos);
        0
    });
}

/// The `klf.host` services: what machine this is, for a program that has to
/// name it — a version manager choosing the artifact a release published for
/// one host.
///
/// The key is the framework's own, so one machine is spelled one way across a
/// toolchain tree: `x86_64-linux-gnu`, `aarch64-macos`, and so on.
fn register_host_services(capabilities: &mut Capabilities) {
    capabilities.register("klfsvc_host_key_length", |_| {
        u64::try_from(klf_target::llvm_host_key().len()).unwrap_or(u64::MAX)
    });
    capabilities.register_with_memory("klfsvc_host_key_copy", |arguments, memory| {
        let [destination, capacity] = arguments else {
            return u64::MAX;
        };
        let bytes = klf_target::llvm_host_key().as_bytes();
        if usize::try_from(*capacity).is_ok_and(|capacity| capacity >= bytes.len())
            && !memory.write_buffer(*destination, bytes)
        {
            return u64::MAX;
        }
        u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    });
}

fn register_process_services(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_process_environment", |arguments, memory| {
        let [name, count, destination, capacity] = arguments else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*count)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if !memory.read_buffer(*name, &mut bytes) {
            return u64::MAX;
        }
        let Ok(name) = std::str::from_utf8(&bytes) else {
            return u64::MAX;
        };
        let Some(value) = std::env::var_os(name) else {
            return u64::MAX;
        };
        let value = value.to_string_lossy();
        let bytes = value.as_bytes();
        if usize::try_from(*capacity).is_ok_and(|capacity| capacity >= bytes.len())
            && !bytes.is_empty()
            && !memory.write_buffer(*destination, bytes)
        {
            return u64::MAX;
        }
        bytes.len() as u64
    });
    capabilities.register("klfsvc_process_argument_count", |_| {
        klf_host_process::argument_count() as u64
    });
    capabilities.register_with_memory("klfsvc_process_argument", |arguments, memory| {
        let [index, destination, capacity] = arguments else {
            return u64::MAX;
        };
        let Some(argument) = usize::try_from(*index)
            .ok()
            .and_then(klf_host_process::argument)
        else {
            return u64::MAX;
        };
        let fits = usize::try_from(*capacity).is_ok_and(|capacity| argument.len() <= capacity);
        if fits && !argument.is_empty() && !memory.write_buffer(*destination, argument) {
            return u64::MAX;
        }
        argument.len() as u64
    });
    capabilities.register_with_memory("klfsvc_process_invoked_as", |arguments, memory| {
        let [destination, capacity] = arguments else {
            return u64::MAX;
        };
        let path = klf_host_process::invoked_as();
        let fits = usize::try_from(*capacity).is_ok_and(|capacity| path.len() <= capacity);
        if fits && !path.is_empty() && !memory.write_buffer(*destination, path) {
            return u64::MAX;
        }
        path.len() as u64
    });
    capabilities.register_with_memory("klfsvc_process_command_push", |arguments, memory| {
        let [data, length] = arguments else {
            return 0;
        };
        let Some(mut bytes) = usize::try_from(*length)
            .ok()
            .filter(|length| *length <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
            .map(|length| vec![0; length])
        else {
            return 0;
        };
        if !bytes.is_empty() && !memory.read_buffer(*data, &mut bytes) {
            return 0;
        }
        u64::from(klf_host_process::command_push(&bytes))
    });
    capabilities.register("klfsvc_process_command_run", |arguments| {
        let captured = matches!(arguments, [1]);
        klf_host_process::command_run(captured)
            .word()
            .cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_process_output", |arguments, memory| {
        let [stream, destination, capacity] = arguments else {
            return u64::MAX;
        };
        klf_host_process::command_output(stream.cast_signed(), |written| {
            let fits = usize::try_from(*capacity).is_ok_and(|capacity| written.len() <= capacity);
            if fits && !written.is_empty() && !memory.write_buffer(*destination, written) {
                return u64::MAX;
            }
            written.len() as u64
        })
        .unwrap_or(u64::MAX)
    });
}
