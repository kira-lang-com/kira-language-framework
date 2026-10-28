//! The definitions a native frontend binary holds (`KLF-KSL.md` section
//! 5.9). A shader toolchain holds its own definition and every bundled
//! one, so that each target's out can run; any other frontend holds one.
//! The registry the backend emitted answers `klffe_metadata` and
//! `klffe_phase` for whichever definition is selected.

use klf_defsvc::FrontendMetadata;
use std::ffi::{CStr, c_char};
use std::path::Path;

unsafe extern "C" {
    fn klffe_language_count() -> i64;
    fn klffe_language_metadata(index: i64) -> *const c_char;
    /// Chooses the definition later calls reach, answering the previous one.
    fn klffe_select(index: i64) -> i64;
}

/// Every definition in the binary, by index.
pub(crate) fn languages() -> Vec<FrontendMetadata> {
    // SAFETY: the registry defines both with these signatures, and each
    // metadata pointer is a NUL-terminated constant of the binary.
    let count = unsafe { klffe_language_count() };
    (0..count)
        .map(|index| {
            let text = unsafe { CStr::from_ptr(klffe_language_metadata(index)) };
            FrontendMetadata::decode(&text.to_string_lossy())
        })
        .collect()
}

pub(crate) fn select(index: i64) -> i64 {
    // SAFETY: the registry defines it; an index it does not hold only makes
    // later calls answer nothing.
    unsafe { klffe_select(index) }
}

/// Selects the definition whose driver the binary was run as, as a link
/// named `kmsl` runs the MSL definition, or else the one it was built for.
pub(crate) fn select_for_program(program: &str) {
    let name = Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let found = languages()
        .iter()
        .position(|language| !language.driver.is_empty() && language.driver == name);
    select(found.map_or(0, |index| index as i64));
}
