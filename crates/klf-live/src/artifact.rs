//! A loaded artifact and the identity a device gives it (section 17.5).
//!
//! A push carries KIR, which is not what any engine runs. The VM lowers it to
//! bytecode and interprets that; the native backends lower it to machine code
//! and write a binary. Either way the device ends up holding something it can
//! run, and that something is what the device must be able to name.
//!
//! Identity is derived here rather than asserted by the client: the device
//! hashes what it built, so two builds of the same bytes share a name and a
//! rebuild that changed anything does not. A native or hybrid device also
//! re-signs the binary it wrote (see [`Signer`](crate::engine::Signer)), and
//! that signature is over these same bytes.

use klf_ridl::sha256::{hex, sha256};
use klf_vm::ForeignSignature;
use std::fmt;

/// What a device built from a pushed module and can run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Artifact {
    /// Bytecode the VM interprets in this process, with the C signature of
    /// each function C calls, which the VM gives a native entry of its own
    /// rather than one built beside the image (section 17.3).
    Bytecode {
        image: Vec<u8>,
        entries: Vec<(String, ForeignSignature)>,
    },
    /// A binary on disk, which a native or hybrid device launches as a process.
    Binary(std::path::PathBuf),
    /// Bytecode plus the native half it calls into, which is what a hybrid
    /// device holds: the two halves share one address space (17.2).
    Hybrid {
        bytecode: Vec<u8>,
        library: std::path::PathBuf,
    },
}

impl Artifact {
    /// The identity the device gives this artifact: what it built, hashed.
    #[must_use]
    pub fn digest(&self) -> String {
        match self {
            Self::Bytecode { image, .. } => hex(&sha256(image)),
            Self::Binary(path) => Self::digest_of(path),
            Self::Hybrid { bytecode, library } => {
                let mut text = hex(&sha256(bytecode));
                text.push(':');
                text.push_str(&Self::digest_of(library));
                text
            }
        }
    }

    /// What a file on disk hashes to, named even when it cannot be read so a
    /// missing file is distinguishable from an empty one.
    fn digest_of(path: &std::path::Path) -> String {
        match std::fs::read(path) {
            Ok(bytes) => hex(&sha256(&bytes)),
            Err(_) => "unreadable".to_owned(),
        }
    }

    /// A short name for logs and acknowledgements.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bytecode { .. } => "bytecode",
            Self::Binary(_) => "binary",
            Self::Hybrid { .. } => "hybrid",
        }
    }

    /// The path a native or hybrid device launches, when this artifact has one.
    #[must_use]
    pub fn program(&self) -> Option<&std::path::Path> {
        match self {
            Self::Bytecode { .. } => None,
            Self::Binary(path) => Some(path),
            Self::Hybrid { library, .. } => Some(library),
        }
    }
}

impl fmt::Display for Artifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.kind(), self.digest())
    }
}

/// How a device was started, and what it is therefore allowed to accept.
///
/// A device with no bundle id installed is a device that accepts nothing: it is
/// not a live device, it is a compiler host that happens to have a socket open.
/// This is the check that keeps section 17.5's "only in debug bundles" from
/// being a flag someone forgets to set.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bundle {
    pub id: String,
    /// Whether the bundle was built for pushing at all. A release bundle
    /// refuses every push whatever else is true of it.
    pub debug: bool,
}

impl Bundle {
    /// Whether this device may accept a push.
    #[must_use]
    pub fn accepts_pushes(&self) -> bool {
        self.debug && !self.id.is_empty()
    }
}

/// One run of a loaded artifact, as a native or hybrid device sees it.
///
/// A VM run is an entry call inside the device's own process. A native or
/// hybrid run is a child process with its own arguments, environment and exit
/// status, which is what "exactly like a normal app" means: the device starts
/// the binary the way a shell would, and the program's `main` gets `argc`,
/// `argv` and `envp` from its own process rather than from a runner's table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunRequest {
    /// The arguments after the program name, which is `argv[1..]`.
    pub arguments: Vec<String>,
    /// The environment as `NAME=value`, which is what a program's `envp` holds.
    pub environment: Vec<String>,
    /// The working directory the program starts in.
    pub directory: Option<std::path::PathBuf>,
}

impl RunRequest {
    /// A run with no arguments, environment or directory, which is what a
    /// program gets when nothing was asked for.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_argument(mut self, argument: impl Into<String>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    #[must_use]
    pub fn with_environment(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment
            .push(format!("{}={}", name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn in_directory(mut self, directory: impl Into<std::path::PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }
}

/// What one run produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// A VM entry call returned this word.
    Returned(u64),
    /// A native or hybrid process exited with this status.
    Exited(i32),
    /// The run could not return: a trap, a failure, or a process that had to
    /// be killed. The text is what a device would report.
    Failed(String),
}

impl RunOutcome {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        match self {
            Self::Returned(_) | Self::Exited(0) => true,
            Self::Exited(_) | Self::Failed(_) => false,
        }
    }
}

impl fmt::Display for RunOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Returned(value) => write!(formatter, "returned {value}"),
            Self::Exited(status) => write!(formatter, "exited {status}"),
            Self::Failed(text) => write!(formatter, "failed: {text}"),
        }
    }
}
