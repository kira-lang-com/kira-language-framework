//! The `klf.process` services that run commands: a program builds one an
//! argument at a time, runs it to its end, then reads what it wrote.

mod children;

/// Kernel service `klf.process.command_push`: appends the `length` bytes at
/// `data` to the command being built. Answers 1, or 0 when they cannot name
/// an argument here.
///
/// # Safety
/// `data` must point at `length` readable bytes, or `length` must be 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_command_push(data: *const u8, length: i64) -> i64 {
    let Ok(count) = usize::try_from(length) else {
        return 0;
    };
    let bytes = if count == 0 || data.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller guarantees `count` readable bytes at `data`.
        unsafe { std::slice::from_raw_parts(data, count) }
    };
    i64::from(klf_host_process::command_push(bytes))
}

/// Kernel service `klf.process.command_run`: runs the command built so far,
/// its output captured when `captured` is 1, and answers its exit status, -1
/// when it ended without one, or -2 when it could not start.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_command_run(captured: i64) -> i64 {
    klf_host_process::command_run(captured == 1).word()
}

/// Kernel service `klf.process.output`: the byte length of what the last
/// command wrote to `stream`, copied to `destination` when it fits in
/// `capacity` bytes; -1 for a stream that is neither 1 nor 2.
///
/// # Safety
/// `destination` must be writable for `capacity` bytes when the output fits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_output(
    stream: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    klf_host_process::command_output(stream, |written| {
        let fits = usize::try_from(capacity).is_ok_and(|capacity| written.len() <= capacity);
        if fits && !written.is_empty() && !destination.is_null() {
            // SAFETY: the output fits in the `capacity` writable bytes at `destination`.
            unsafe { std::ptr::copy_nonoverlapping(written.as_ptr(), destination, written.len()) };
        }
        i64::try_from(written.len()).unwrap_or(i64::MAX)
    })
    .unwrap_or(-1)
}
