use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use klf_kir::Module;

static STAGED_ARTIFACT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn linked_binary_cache(
    module: &Module,
    language: &str,
    entry: klf_services::EntryConvention,
    level: klf_kir_passes::Level,
    target: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(root) = klf_target::cache_root() else {
        return Ok(None);
    };
    let Some(archives) = super::linked_archives(module, language, target) else {
        return Ok(None);
    };
    let mut hash = blake3::Hasher::new();
    hash_field(&mut hash, b"klf-native-output-v1");
    hash_field(&mut hash, env!("CARGO_PKG_VERSION").as_bytes());
    hash_field(&mut hash, language.as_bytes());
    hash_field(&mut hash, target.as_bytes());
    hash_field(&mut hash, format!("{entry:?}:{level:?}").as_bytes());
    {
        let mut writer = HashWriter(&mut hash);
        write!(&mut writer, "{module}").map_err(|_| "cannot fingerprint KIR module".to_owned())?;
    }
    hash_field(&mut hash, &[]);
    for name in [
        "KLF_USE_SYSTEM_LINKER",
        "CC",
        "CLANG",
        "KLF_CLANG",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
    ] {
        hash_field(&mut hash, name.as_bytes());
        if let Some(value) = std::env::var_os(name) {
            hash_field(&mut hash, value.as_encoded_bytes());
        } else {
            hash_field(&mut hash, &[]);
        }
    }
    if let Some(clang) = super::find_tool("clang") {
        hash_field(&mut hash, clang.as_os_str().as_encoded_bytes());
        hash_file_metadata(&mut hash, &clang)?;
    }
    for archive in archives {
        hash_file(&mut hash, &archive)?;
    }
    for library in super::system_libraries(target) {
        hash_field(&mut hash, library.as_bytes());
    }
    Ok(Some(
        root.join("native-output")
            .join(hash.finalize().to_hex().as_str()),
    ))
}

fn hash_file(hash: &mut blake3::Hasher, path: &Path) -> Result<(), String> {
    hash_field(hash, path.as_os_str().as_encoded_bytes());
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("cannot read link input {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect link input {}: {error}", path.display()))?;
    hash_field(hash, &metadata.len().to_le_bytes());
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read link input {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    hash_field(hash, &[]);
    Ok(())
}

fn hash_file_metadata(hash: &mut blake3::Hasher, path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot inspect linker {}: {error}", path.display()))?;
    hash_field(hash, &metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified() {
        if let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) {
            hash_field(hash, &since_epoch.as_nanos().to_le_bytes());
        }
    }
    Ok(())
}

fn hash_field(hash: &mut blake3::Hasher, value: &[u8]) {
    hash.update(&(value.len() as u64).to_le_bytes());
    hash.update(value);
}

struct HashWriter<'a>(&'a mut blake3::Hasher);

impl std::fmt::Write for HashWriter<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.0.update(value.as_bytes());
        Ok(())
    }
}

pub(super) fn cache_linked_binary(output: &Path, cache: &Path) -> Result<(), String> {
    if cache.is_file() {
        return Ok(());
    }
    let parent = cache
        .parent()
        .ok_or_else(|| format!("{} has no cache directory", cache.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let staged = staged_path(cache);
    if std::fs::hard_link(output, &staged).is_err() {
        std::fs::copy(output, &staged)
            .map_err(|error| format!("cannot cache {}: {error}", output.display()))?;
    }
    match std::fs::rename(&staged, cache) {
        Ok(()) => Ok(()),
        Err(_) if cache.is_file() => {
            let _ = std::fs::remove_file(staged);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(staged);
            Err(format!("cannot install {}: {error}", cache.display()))
        }
    }
}

pub(super) fn publish_cached_binary(cache: &Path, output: &Path) -> Result<(), String> {
    if same_file(cache, output) {
        return Ok(());
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let staged = staged_path(output);
    if std::fs::hard_link(cache, &staged).is_err() {
        std::fs::copy(cache, &staged)
            .map_err(|error| format!("cannot publish {}: {error}", cache.display()))?;
    }
    replace_file(&staged, output)
}

fn staged_path(path: &Path) -> PathBuf {
    let index = STAGED_ARTIFACT.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("klf-{}-{index}.tmp", std::process::id()))
}

fn same_file(first: &Path, second: &Path) -> bool {
    let Ok(first_metadata) = std::fs::metadata(first) else {
        return false;
    };
    let Ok(second_metadata) = std::fs::metadata(second) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        first_metadata.dev() == second_metadata.dev()
            && first_metadata.ino() == second_metadata.ino()
    }
    #[cfg(not(unix))]
    {
        first_metadata.len() == second_metadata.len()
            && first_metadata.modified().ok() == second_metadata.modified().ok()
    }
}

fn replace_file(staged: &Path, output: &Path) -> Result<(), String> {
    match std::fs::rename(staged, output) {
        Ok(()) => Ok(()),
        Err(_) if output.exists() => {
            std::fs::remove_file(output)
                .map_err(|error| format!("cannot replace {}: {error}", output.display()))?;
            std::fs::rename(staged, output)
                .map_err(|error| format!("cannot install {}: {error}", output.display()))
        }
        Err(error) => Err(format!("cannot install {}: {error}", output.display())),
    }
}
