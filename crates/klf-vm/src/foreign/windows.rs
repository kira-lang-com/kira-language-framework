//! Native libraries on a Windows host: loaded with `LoadLibraryW`, searched
//! with `GetProcAddress`, and named with `GetModuleFileNameW`.
#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_void};
use std::os::windows::ffi::OsStrExt;

type Module = *mut c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(path: *const u16) -> Module;
    fn GetProcAddress(module: Module, symbol: *const c_char) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Module) -> i32;
    fn GetModuleFileNameW(module: Module, path: *mut u16, size: u32) -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn K32EnumProcessModules(
        process: *mut c_void,
        modules: *mut Module,
        bytes: u32,
        needed: *mut u32,
    ) -> i32;
}

/// `GetModuleHandleExW` flags: the name is an address inside the module,
/// and the module's reference count stays as it is.
const FROM_ADDRESS: u32 = 0x4;
const UNCHANGED_REFCOUNT: u32 = 0x2;
/// The longest path `GetModuleFileNameW` writes, in UTF-16 units.
const LONGEST_PATH: usize = 32_768;

/// The address of `symbol` in the first of `libraries` that defines it, or
/// else in a module the process has loaded, the program's own first. Every
/// program's native library defines the same helper names, so a program
/// finds its own before another's.
pub fn resolve(libraries: &[usize], symbol: &str) -> Option<usize> {
    let name = CString::new(symbol).ok()?;
    libraries
        .iter()
        .map(|library| *library as Module)
        .chain(process_modules())
        // SAFETY: every module is one the process has loaded.
        .map(|module| unsafe { GetProcAddress(module, name.as_ptr()) })
        .find(|address| !address.is_null())
        .map(|address| address as usize)
}

/// Every module the process has loaded, the executable first, as the
/// process scope a Unix host searches through `RTLD_DEFAULT`.
fn process_modules() -> Vec<Module> {
    let mut modules: Vec<Module> = Vec::new();
    // A module loaded between the two calls is missed, which is no worse
    // than looking a moment earlier.
    for _ in 0..2 {
        let mut needed = 0;
        let bytes = u32::try_from(std::mem::size_of_val(modules.as_slice())).unwrap_or(u32::MAX);
        // SAFETY: `modules` holds `bytes` writable bytes.
        let listed = unsafe {
            K32EnumProcessModules(
                GetCurrentProcess(),
                modules.as_mut_ptr(),
                bytes,
                &raw mut needed,
            )
        };
        if listed == 0 {
            return Vec::new();
        }
        let count = needed as usize / std::mem::size_of::<Module>();
        if count <= modules.len() {
            modules.truncate(count);
            return modules;
        }
        modules.resize(count, std::ptr::null_mut());
    }
    modules
}

/// The path of the loaded image holding `address`.
pub fn image_of(address: usize) -> Option<String> {
    let mut module: Module = std::ptr::null_mut();
    // SAFETY: with `FROM_ADDRESS` the name is read as an address only.
    let found = unsafe {
        GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            address as *const u16,
            &raw mut module,
        )
    };
    if found == 0 {
        return None;
    }
    let mut path = vec![0u16; LONGEST_PATH];
    let size = u32::try_from(path.len()).unwrap_or(u32::MAX);
    // SAFETY: `path` holds `size` writable units.
    let written = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), size) } as usize;
    if written == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&path[..written]))
}

/// Loads the library at `path` and answers its handle.
pub fn load(path: &str) -> Result<usize, String> {
    if path.contains('\0') {
        return Err(format!("library path `{path}` holds a NUL"));
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path.
    let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
    if handle.is_null() {
        let reason = std::io::Error::last_os_error();
        return Err(format!("cannot load {path}: {reason}"));
    }
    Ok(handle as usize)
}
