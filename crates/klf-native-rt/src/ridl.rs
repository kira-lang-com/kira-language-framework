//! RIDL module registration and link checks (section 24.5).
//!
//! Every library with a RIDL surface registers its metadata blob from a
//! constructor. A module's imports are checked against providers as both
//! register, whichever comes first.

use klf_ridl::meta::{LinkCheck, Metadata, check_import, conflicting_current};
use std::sync::Mutex;

static MODULES: Mutex<Vec<Metadata>> = Mutex::new(Vec::new());

fn strict() -> bool {
    std::env::var("KLF_RIDL_STRICT").is_ok_and(|value| value == "1")
}

fn hex(uuid: &[u8]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reports every import of `importer` that `provider` fails, and ends the
/// process for a fatal one.
fn check(importer: &Metadata, provider: &Metadata) {
    for import in importer
        .imports
        .iter()
        .filter(|import| import.module == provider.module_name)
    {
        match check_import(provider, &import.required, strict()) {
            LinkCheck::Ok => {}
            LinkCheck::RetiredWarning { proceed } => {
                eprintln!(
                    "warning KLF-RT-0201: {} requires retired ABI {} of {}",
                    importer.module_name,
                    hex(&import.required),
                    provider.module_name
                );
                if !proceed {
                    std::process::exit(1);
                }
            }
            LinkCheck::Missing => {
                eprintln!(
                    "runtime error KLF-RT-0200: {} requires ABI {} of {}, which provides {}",
                    importer.module_name,
                    hex(&import.required),
                    provider.module_name,
                    provider
                        .current_uuid()
                        .map_or_else(|| "none".to_owned(), |uuid| hex(&uuid))
                );
                std::process::exit(1);
            }
        }
    }
}

/// Registers the RIDL metadata blob of a loaded module.
///
/// # Safety
/// `blob` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfrt_ridl_register(blob: *const u8, length: usize) {
    // SAFETY: the caller passes its own static metadata section.
    let bytes = unsafe { std::slice::from_raw_parts(blob, length) };
    let metadata = match Metadata::decode(bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("runtime error KLF-RT-0200: unreadable RIDL metadata: {error}");
            std::process::exit(1);
        }
    };
    let mut modules = MODULES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for loaded in modules.iter() {
        if conflicting_current(loaded, &metadata) {
            eprintln!(
                "runtime error KLF-RT-0202: two copies of {} with different current ABIs are loaded",
                metadata.module_name
            );
            std::process::exit(1);
        }
        check(&metadata, loaded);
        check(loaded, &metadata);
    }
    modules.push(metadata);
}
