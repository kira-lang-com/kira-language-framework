//! Read-only process environment access for Foundation.

/// Copy a variable's UTF-8 value when the destination fits, returning its length
/// or -1 when absent.
///
/// # Safety
/// `name` contains `name_count` readable bytes. `destination` contains `capacity`
/// writable bytes when the value fits and is nonempty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_process_environment(
    name: *const u8,
    name_count: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Ok(name_count) = usize::try_from(name_count) else {
        return -1;
    };
    if name.is_null() || name_count == 0 {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name, name_count) };
    let Ok(name) = std::str::from_utf8(bytes) else {
        return -1;
    };
    let Some(value) = std::env::var_os(name) else {
        return -1;
    };
    let value = value.to_string_lossy();
    let bytes = value.as_bytes();
    if usize::try_from(capacity).is_ok_and(|capacity| capacity >= bytes.len()) && !bytes.is_empty()
    {
        if destination.is_null() {
            return -1;
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len()) };
    }
    i64::try_from(bytes.len()).unwrap_or(-1)
}
