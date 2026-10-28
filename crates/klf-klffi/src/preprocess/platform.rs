//! The macros a C compiler predefines for a target. A header's `#if` lines
//! test them, so the set is an explicit input to the preprocessor.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
    Tessera,
}

impl Platform {
    /// The platform a target triple names, or `None` when it names none of
    /// the supported ones.
    #[must_use]
    pub fn from_triple(triple: &str) -> Option<Self> {
        if triple.contains("windows") {
            Some(Self::Windows)
        } else if triple.contains("apple") || triple.contains("darwin") {
            Some(Self::MacOs)
        } else if triple.contains("linux") {
            Some(Self::Linux)
        } else if triple.contains("tessera") {
            Some(Self::Tessera)
        } else {
            None
        }
    }
}

/// The predefined macros of one target, as `(name, value)` pairs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Predefined {
    definitions: Vec<(String, String)>,
}

impl Predefined {
    /// The macros a compiler for `triple` predefines, or `None` when the
    /// architecture or the operating system of `triple` is not known.
    #[must_use]
    pub fn for_triple(triple: &str) -> Option<Self> {
        let architecture = triple.split('-').next()?;
        let (architecture_macros, pointer_bits) = architecture_macros(architecture)?;
        let platform = Platform::from_triple(triple)?;
        let mut definitions = pairs(&[("__STDC__", "1"), ("__STDC_VERSION__", "201710L")]);
        definitions.extend(pairs(architecture_macros));
        if pointer_bits == 64 {
            definitions.extend(pairs(&[("__LP64__", "1"), ("_LP64", "1")]));
        }
        definitions.extend(platform_macros(platform, pointer_bits));
        Some(Self { definitions })
    }

    pub(crate) fn definitions(&self) -> &[(String, String)] {
        &self.definitions
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn architecture_macros(architecture: &str) -> Option<(&'static [(&'static str, &'static str)], u8)> {
    Some(match architecture {
        "x86_64" => (&[("__x86_64__", "1"), ("__amd64__", "1")], 64),
        "aarch64" => (&[("__aarch64__", "1")], 64),
        "riscv64" => (&[("__riscv", "1"), ("__riscv_xlen", "64")], 64),
        "riscv32" => (&[("__riscv", "1"), ("__riscv_xlen", "32")], 32),
        "i386" | "i486" | "i586" | "i686" => (&[("__i386__", "1")], 32),
        _ => return None,
    })
}

fn platform_macros(platform: Platform, pointer_bits: u8) -> Vec<(String, String)> {
    match platform {
        Platform::Windows => {
            let mut macros = pairs(&[("_WIN32", "1")]);
            if pointer_bits == 64 {
                macros.extend(pairs(&[("_WIN64", "1")]));
            }
            macros
        }
        Platform::Linux => pairs(&[
            ("__linux__", "1"),
            ("__linux", "1"),
            ("__unix__", "1"),
            ("__unix", "1"),
            ("__ELF__", "1"),
        ]),
        Platform::MacOs => pairs(&[("__APPLE__", "1"), ("__MACH__", "1")]),
        Platform::Tessera => Vec::new(),
    }
}
