use std::path::PathBuf;
use std::process::ExitCode;

pub fn command(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        return fail("usage: klf clean");
    }
    let Some(cache) = klf_target::cache_root() else {
        return fail("set KLF_CACHE or HOME before cleaning the KLF cache");
    };
    let metadata = match std::fs::symlink_metadata(&cache) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            klf_diag::out!("KLF cache is empty");
            return ExitCode::SUCCESS;
        }
        Err(error) => return fail(&format!("cannot inspect `{}`: {error}", cache.display())),
    };
    let canonical = match cache.canonicalize() {
        Ok(path) => path,
        Err(error) => return fail(&format!("cannot resolve `{}`: {error}", cache.display())),
    };
    if unsafe_target(&canonical) {
        return fail(&format!(
            "refusing to remove broad path `{}`",
            cache.display()
        ));
    }

    let result = if metadata.file_type().is_symlink() || !metadata.is_dir() {
        std::fs::remove_file(&cache)
    } else {
        std::fs::remove_dir_all(&cache)
    };
    match result {
        Ok(()) => {
            klf_diag::out!("removed KLF cache `{}`", cache.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("cannot remove `{}`: {error}", cache.display())),
    }
}

fn unsafe_target(path: &std::path::Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    let mut protected = Vec::new();
    if let Some(current) = std::env::current_dir()
        .ok()
        .and_then(|current| current.canonicalize().ok())
    {
        protected.push(current);
    }
    if let Some(home) =
        std::env::var_os("HOME").and_then(|home| PathBuf::from(home).canonicalize().ok())
    {
        protected.push(home);
    }
    if let Ok(temp) = std::env::temp_dir().canonicalize() {
        protected.push(temp);
    }
    for directory in ["/tmp", "/var/tmp"] {
        if let Ok(temp) = PathBuf::from(directory).canonicalize() {
            protected.push(temp);
        }
    }
    protected
        .iter()
        .any(|directory| directory.starts_with(path))
}

fn fail(message: &str) -> ExitCode {
    klf_diag::err!("klf clean: {message}");
    ExitCode::FAILURE
}
