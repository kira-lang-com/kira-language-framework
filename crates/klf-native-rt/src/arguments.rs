//! `klf.process.argument_count`, `klf.process.argument` and
//! `klf.process.invoked_as`: what a program was started with.

/// Kernel service `klf.process.argument_count`: how many arguments the
/// program was started with, its path excluded.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_argument_count() -> i64 {
    i64::try_from(klf_host_process::argument_count()).unwrap_or(i64::MAX)
}

/// Kernel service `klf.process.argument`: the byte length of argument
/// `index`, copied to `destination` when it fits in `capacity` bytes, or -1
/// past the last argument.
///
/// # Safety
///
/// `destination` must be writable for `capacity` bytes when the argument fits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_argument(
    index: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(argument) = usize::try_from(index)
        .ok()
        .and_then(klf_host_process::argument)
    else {
        return -1;
    };
    let fits = usize::try_from(capacity).is_ok_and(|capacity| argument.len() <= capacity);
    if fits && !argument.is_empty() && !destination.is_null() {
        // SAFETY: the argument fits in the `capacity` writable bytes at `destination`.
        unsafe { std::ptr::copy_nonoverlapping(argument.as_ptr(), destination, argument.len()) };
    }
    i64::try_from(argument.len()).unwrap_or(i64::MAX)
}

/// Kernel service `klf.process.invoked_as`: the byte length of the path the
/// program was started as, copied to `destination` when it fits in
/// `capacity` bytes.
///
/// # Safety
///
/// `destination` must be writable for `capacity` bytes when the path fits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_invoked_as(destination: *mut u8, capacity: i64) -> i64 {
    let path = klf_host_process::invoked_as();
    let fits = usize::try_from(capacity).is_ok_and(|capacity| path.len() <= capacity);
    if fits && !path.is_empty() && !destination.is_null() {
        // SAFETY: the path fits in the `capacity` writable bytes at `destination`.
        unsafe { std::ptr::copy_nonoverlapping(path.as_ptr(), destination, path.len()) };
    }
    i64::try_from(path.len()).unwrap_or(i64::MAX)
}
