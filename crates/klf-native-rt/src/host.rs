//! What host a program is running on, for a program that has to name it.
//!
//! A version manager is the case this exists for: it fetches an artifact the
//! release published for one machine, and cannot choose which without being told
//! what machine it is on. The key is the same `<arch>-<os>` string the framework
//! names its own provisioned bundles by, so one machine is spelled one way across
//! the whole toolchain tree.
//!
//! A length and a copy rather than one call, because the key has no fixed size
//! and a program hands these services a buffer it owns. Same shape as
//! `klf.text.float` and `klf.process.argument`.

/// Kernel service `klf.host.key_length`: the byte length of this host's key.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_host_key_length() -> i64 {
    i64::try_from(klf_target::llvm_host_key().len()).unwrap_or(i64::MAX)
}

/// Kernel service `klf.host.key_copy`: this host's key, copied to `destination`
/// when it fits in `capacity` bytes.
///
/// # Safety
///
/// `destination` must be writable for `capacity` bytes when the key fits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_host_key_copy(destination: *mut u8, capacity: i64) -> i64 {
    let bytes = klf_target::llvm_host_key().as_bytes();
    let fits = usize::try_from(capacity).is_ok_and(|capacity| capacity >= bytes.len());
    if fits && !bytes.is_empty() {
        if destination.is_null() {
            return -1;
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len()) };
    }
    i64::try_from(bytes.len()).unwrap_or(-1)
}
