use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A directory for one output's intermediate files, removed with it.
pub(crate) struct Scratch(PathBuf);

static NEXT: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    pub(crate) fn new() -> Result<Self, String> {
        // A compiler output must own an exclusively created scratch directory.
        // create_dir_all would reuse a stale directory or an attacker-chosen
        // symlink at this predictable name, then Drop could delete its files.
        for _ in 0..64 {
            let directory = std::env::temp_dir().join(format!(
                "klf-frontend-rt-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&directory) {
                Ok(()) => return Ok(Self(directory)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("cannot create {}: {error}", directory.display()));
                }
            }
        }
        Err("cannot reserve a unique compiler scratch directory".to_owned())
    }

    /// The directory itself.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        // Intermediate names must remain immediately inside their own
        // directory. Do not allow `..`, absolute paths or nested names.
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(format!("invalid scratch filename: {name}"));
        }
        let path = self.0.join(name);
        std::fs::write(&path, bytes)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::Scratch;

    #[test]
    fn scratch_outputs_are_isolated_and_cleaned_up() {
        let first = Scratch::new().unwrap();
        let second = Scratch::new().unwrap();
        assert_ne!(first.path(), second.path());
        let first_path = first.write("program.o", b"one").unwrap();
        let second_path = second.write("program.o", b"two").unwrap();
        assert_eq!(std::fs::read(&first_path).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(first.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "scratch directory must be private");
        }
        assert_eq!(std::fs::read(&second_path).unwrap(), b"two");
        let dir = first.path().to_path_buf();
        drop(first);
        assert!(!dir.exists());
        assert_eq!(std::fs::read(&second_path).unwrap(), b"two");
    }

    #[test]
    fn scratch_rejects_traversal_and_absolute_output_names() {
        let scratch = Scratch::new().unwrap();
        for name in ["../escape", "sub/file", "/tmp/escape", ".", "..", ""] {
            assert!(
                scratch.write(name, b"should not write").is_err(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn scratch_does_not_reuse_an_existing_directory() {
        use super::NEXT;
        use std::sync::atomic::Ordering;

        // Occupy the exact predictable slot the next compiler output would
        // request. The previous create_dir_all implementation reused it.
        let index = NEXT.load(Ordering::Relaxed);
        let occupied =
            std::env::temp_dir().join(format!("klf-frontend-rt-{}-{index}", std::process::id()));
        std::fs::create_dir(&occupied).unwrap();
        let sentinel = occupied.join("original");
        std::fs::write(&sentinel, b"do not erase").unwrap();
        let scratch = Scratch::new().unwrap();
        assert_ne!(scratch.path(), occupied);
        drop(scratch);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"do not erase");
        std::fs::remove_dir_all(&occupied).unwrap();
    }
}
