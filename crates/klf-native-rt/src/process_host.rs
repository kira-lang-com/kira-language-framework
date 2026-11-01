//! Process state shared with native libraries attached to the executable.

/// The process entry and trap policy visible to this runtime instance.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_process_mode() -> i32 {
    crate::klfrt_process_active() | state()
}

#[cfg(windows)]
pub(crate) fn state() -> i32 {
    // Windows bytecode native libraries link their own runtime. The host may
    // enter process mode after the library is loaded, so query it on each call.
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
        fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *mut core::ffi::c_void;
    }
    let entry = unsafe { GetProcAddress(GetModuleHandleW(std::ptr::null()), c"klfrt_process_active".as_ptr().cast()) };
    if entry.is_null() {
        return 0;
    }
    let active: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(entry) };
    unsafe { active() }
}

#[cfg(not(windows))]
pub(crate) fn state() -> i32 {
    0
}
