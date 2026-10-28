//! Native libraries on a Unix host: loaded with `dlopen`, searched with
//! `dlsym`, and named with `dladdr`.
#![allow(unsafe_code)]

use std::ffi::{CString, c_void};
use std::os::raw::{c_char, c_int};

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
}

#[repr(C)]
struct DlInfo {
    file: *const c_char,
    base: *mut c_void,
    symbol: *const c_char,
    address: *mut c_void,
}

#[cfg(target_vendor = "apple")]
const RTLD_DEFAULT: *mut c_void = (-2isize) as *mut c_void;
#[cfg(not(target_vendor = "apple"))]
const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();
const RTLD_NOW: c_int = 2;
#[cfg(target_vendor = "apple")]
const RTLD_GLOBAL: c_int = 8;
#[cfg(not(target_vendor = "apple"))]
const RTLD_GLOBAL: c_int = 0x100;

/// The address of `symbol` in the first of `libraries` that defines it,
/// each searched with the libraries it depends on, or else in the
/// process. Every program's native library defines the same helper
/// names, so a program finds its own before another's.
pub fn resolve(libraries: &[usize], symbol: &str) -> Option<usize> {
    let name = CString::new(symbol).ok()?;
    libraries
        .iter()
        .map(|library| *library as *mut c_void)
        .chain(std::iter::once(RTLD_DEFAULT))
        // SAFETY: every handle is a loaded library or the default scope.
        .map(|handle| unsafe { dlsym(handle, name.as_ptr()) })
        .find(|address| !address.is_null())
        .map(|address| address as usize)
}

/// The path of the loaded image holding `address`.
pub fn image_of(address: usize) -> Option<String> {
    let mut info = DlInfo {
        file: std::ptr::null(),
        base: std::ptr::null_mut(),
        symbol: std::ptr::null(),
        address: std::ptr::null_mut(),
    };
    let found = unsafe { dladdr(address as *const c_void, &raw mut info) };
    if found == 0 || info.file.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(info.file) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Loads the library at `path` into the process's global scope and
/// answers its handle.
pub fn load(path: &str) -> Result<usize, String> {
    let name = CString::new(path).map_err(|_| format!("library path `{path}` holds a NUL"))?;
    let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
    if handle.is_null() {
        let reason = unsafe { dlerror() };
        let reason = if reason.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(reason) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(format!("cannot load {path}: {reason}"));
    }
    Ok(handle as usize)
}
