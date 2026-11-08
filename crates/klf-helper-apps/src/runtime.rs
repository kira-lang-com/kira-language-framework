#![allow(unsafe_code)]

use crate::callbacks::Callbacks;
use crate::managed::{HostedApp, ManagedApp};
use klf_ridl::meta::{LinkCheck, Metadata, check_import};
use klf_ridl_wire::transport::{Connection, RemoteObject};
use klf_ridl_wire::{Reply, Value};
use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const CHANNEL_FD: RawFd = 3;
const CHANNEL_ENV: &str = "KLF_RIDL_CHANNEL_FD";
const ID_ENV: &str = "KLF_RIDL_CONNECTION_ID";
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
static CONNECTION_ADOPTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PluginId(String);

impl PluginId {
    pub fn new(value: impl Into<String>) -> io::Result<Self> {
        let value = value.into();
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(invalid("invalid installed application identifier"));
        }
        Ok(Self(value))
    }
}

/// A built Kira application. Hybrid bundles carry their own VM image;
/// bytecode applications run in the platform's installed Kira VM runner.
#[derive(Clone, Debug)]
pub enum Artifact {
    Native {
        executable: PathBuf,
    },
    Vm {
        image: PathBuf,
        /// The compiler's `.kirb` retains its generated `@Ridl` import map.
        kir: PathBuf,
    },
}

impl Artifact {
    fn canonicalize(&mut self) -> io::Result<()> {
        match self {
            Self::Native { executable } => *executable = executable.canonicalize()?,
            Self::Vm { image, kir } => {
                *image = image.canonicalize()?;
                *kir = kir.canonicalize()?;
            }
        }
        Ok(())
    }

    fn command(&self, vm_runner: Option<&Path>) -> io::Result<Command> {
        Ok(match self {
            Self::Native { executable } => Command::new(executable),
            Self::Vm { image, kir } => {
                let runner = vm_runner.ok_or_else(|| {
                    io::Error::new(ErrorKind::NotFound, "host has no Kira VM helper runtime")
                })?;
                let mut command = Command::new(runner);
                command.arg(image);
                command.arg(kir);
                command
            }
        })
    }

    fn is_installed(&self) -> bool {
        match self {
            Self::Native { executable } => executable.is_file(),
            Self::Vm { image, kir } => image.is_file() && kir.is_file(),
        }
    }
}

/// Describes an installed, already-built helper or marketplace plugin.
/// Metadata is the RIDL blob produced by the build, not an alternative schema.
#[derive(Clone, Debug)]
pub struct InstalledApp {
    pub artifact: Artifact,
    pub metadata: Metadata,
}

impl InstalledApp {
    /// Reads the compiler's RIDL metadata alongside an installed Kira artifact.
    /// No second plugin metadata format is needed for runtime ABI binding.
    ///
    /// # Errors
    /// Returns an error when the metadata is missing, invalid, or inconsistent
    /// with the VM module's generated RIDL import declarations.
    pub fn from_compiled(artifact: Artifact, metadata: impl AsRef<Path>) -> io::Result<Self> {
        let bytes = std::fs::read(metadata)?;
        let metadata = Metadata::decode(&bytes).map_err(|error| invalid(&error.to_string()))?;
        let app = Self { artifact, metadata };
        app.verify_imports()?;
        Ok(app)
    }

    fn verify_imports(&self) -> io::Result<(Option<HashSet<String>>, Option<[u8; 32]>)> {
        let Artifact::Vm { kir, .. } = &self.artifact else {
            return Ok((None, None));
        };
        let bytes = std::fs::read(kir)?;
        let fingerprint = klf_ridl::sha256::sha256(&bytes);
        let module = klf_kir::kirb::decode(&bytes)
            .ok_or_else(|| invalid("invalid compiled Kira RIDL import map"))?;
        let mut declarations = HashSet::new();
        let mut declared_paths = HashSet::new();
        for attribute in &module.attributes {
            if attribute.name != "ridl.import" {
                continue;
            }
            let (name, body) = attribute
                .value
                .as_deref()
                .and_then(|text| text.split_once('\t'))
                .ok_or_else(|| invalid("malformed generated RIDL import contract"))?;
            let surface = klf_ridl::parse(klf_core::FileId::from_u32(0), name, body)
                .map_err(|error| invalid(&format!("invalid generated RIDL import: {error}")))?;
            declarations.insert(name.to_owned());
            for item in &surface.items {
                match &item.kind {
                    klf_ridl::module::ItemKind::Function(function) => {
                        declared_paths.insert(format!("{name}.{}", function.name));
                    }
                    klf_ridl::module::ItemKind::Extend(extend) => {
                        if let (
                            klf_ridl::types::Type::Nominal { name: receiver, .. },
                            klf_ridl::module::ExtendBody::Methods(methods),
                        ) = (&extend.target, &extend.body)
                        {
                            for method in methods {
                                declared_paths.insert(format!("{name}.{receiver}.{}", method.name));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut authorized = HashSet::new();
        for import in &self.metadata.imports {
            if !authorized.insert(import.module.clone()) {
                return Err(invalid("duplicate RIDL imported module in metadata"));
            }
        }
        if declarations != authorized {
            return Err(invalid(
                "compiled RIDL imports disagree with installed metadata",
            ));
        }
        let mut permitted = HashSet::new();
        for attribute in &module.attributes {
            if attribute.name != "ridl.import.entry" {
                continue;
            }
            let Some(record) = attribute.value.as_deref() else {
                return Err(invalid("empty generated RIDL entry"));
            };
            let mut parts = record.split('\t');
            let (Some(name), Some(path), Some(_symbol), Some(_shape), None) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return Err(invalid("malformed generated RIDL entry"));
            };
            if !declarations.contains(name) {
                return Err(invalid("RIDL entry refers to an unimported host module"));
            }
            let name = format!("{name}.{path}");
            if !declared_paths.contains(&name) {
                return Err(invalid("RIDL entry is absent from its imported contract"));
            }
            permitted.insert(name);
        }
        Ok((Some(permitted), Some(fingerprint)))
    }
}

struct CatalogEntry {
    app: InstalledApp,
    kir_fingerprint: Option<[u8; 32]>,
}

/// Installed IDs are resolved by this registry; no application supplies an
/// executable name, command line, or socket name to start a plugin.
#[derive(Default)]
pub struct Catalog {
    apps: HashMap<PluginId, CatalogEntry>,
    vm_runner: Option<PathBuf>,
}

impl Catalog {
    /// The host runtime chooses the VM executor. Marketplace packages cannot
    /// substitute an executable through their plugin artifact declaration.
    pub fn set_vm_runtime(&mut self, runner: PathBuf) -> io::Result<()> {
        if !runner.is_file() {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                "Kira VM helper runtime not found",
            ));
        }
        self.vm_runner = Some(runner.canonicalize()?);
        Ok(())
    }

    pub fn install(&mut self, id: PluginId, app: InstalledApp) -> io::Result<()> {
        if matches!(app.artifact, Artifact::Native { .. }) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "native/hybrid Kira plugins need explicit trust until OS sandboxing is available",
            ));
        }
        self.install_internal(id, app)
    }

    /// Installs a native/hybrid app trusted by the embedding host. Marketplace
    /// installs must use `install` until OS sandboxing is implemented.
    pub fn install_trusted_native(&mut self, id: PluginId, app: InstalledApp) -> io::Result<()> {
        self.install_internal(id, app)
    }

    fn install_internal(&mut self, id: PluginId, mut app: InstalledApp) -> io::Result<()> {
        if !app.artifact.is_installed() {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                "installed Kira artifact not found",
            ));
        }
        let (_, kir_fingerprint) = app.verify_imports()?;
        app.artifact.canonicalize()?;
        if self.apps.contains_key(&id) {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                "application is already installed",
            ));
        }
        self.apps.insert(
            id,
            CatalogEntry {
                app,
                kir_fingerprint,
            },
        );
        Ok(())
    }

    pub fn get(&self, id: &PluginId) -> Option<&InstalledApp> {
        self.apps.get(id).map(|entry| &entry.app)
    }

    fn expected_kir_fingerprint(&self, id: &PluginId) -> Option<[u8; 32]> {
        self.apps.get(id).and_then(|entry| entry.kir_fingerprint)
    }
}

pub type Service = Arc<dyn Fn(&[Value]) -> Reply + Send + Sync>;

struct ExportedModule {
    metadata: Metadata,
    functions: HashMap<String, Service>,
}

/// Host-owned RIDL services. Plugin imports are checked against ABI metadata
/// before the runtime starts its separate process.
#[derive(Default)]
pub struct Host {
    modules: HashMap<String, ExportedModule>,
    pub(crate) callbacks: Arc<Callbacks>,
}

impl Host {
    pub fn export(
        &mut self,
        metadata: Metadata,
        functions: HashMap<String, Service>,
    ) -> io::Result<()> {
        if self.modules.contains_key(&metadata.module_name) {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                "RIDL module already exported",
            ));
        }
        self.modules.insert(
            metadata.module_name.clone(),
            ExportedModule {
                metadata,
                functions,
            },
        );
        Ok(())
    }

    pub fn start(&self, catalog: &Catalog, id: &PluginId) -> io::Result<HostedApp> {
        let app = catalog
            .get(id)
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "plugin is not installed"))?;
        let (permitted, fingerprint) = app.verify_imports()?;
        if fingerprint != catalog.expected_kir_fingerprint(id) {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                "Kira plugin RIDL imports changed after installation",
            ));
        }
        let mut routes = HashMap::new();
        for import in &app.metadata.imports {
            let provider = self.modules.get(&import.module).ok_or_else(|| {
                invalid(&format!(
                    "KLF-RT-0200: missing RIDL module {}",
                    import.module
                ))
            })?;
            let strict = std::env::var_os("KLF_RIDL_STRICT").is_some_and(|value| value == "1");
            match check_import(&provider.metadata, &import.required, strict) {
                LinkCheck::Ok => {}
                LinkCheck::RetiredWarning { proceed: true } => {
                    eprintln!("KLF-RT-0201: {} uses retired RIDL ABI", import.module);
                }
                _ => {
                    return Err(invalid(&format!(
                        "KLF-RT-0200: incompatible RIDL ABI for {}",
                        import.module
                    )));
                }
            }
            for (name, handler) in &provider.functions {
                let path = format!("{}.{}", import.module, name);
                if permitted
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&path))
                {
                    routes.insert(path, Arc::clone(handler));
                }
            }
        }
        let identity = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
        let (connection, child_endpoint) = UnixStream::pair()?;
        let mut command = app.artifact.command(catalog.vm_runner.as_deref())?;
        command
            .env_clear()
            .env(CHANNEL_ENV, CHANNEL_FD.to_string())
            .env(ID_ENV, identity.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child_fd = child_endpoint.as_raw_fd();
        // SAFETY: the closure performs only async-signal-safe syscalls between
        // fork and exec. This descriptor is private to the new child.
        unsafe {
            command.pre_exec(move || {
                if child_fd != CHANNEL_FD && libc::dup2(child_fd, CHANNEL_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(CHANNEL_FD, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(CHANNEL_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop(child_endpoint);
        Ok(HostedApp {
            child: Mutex::new(child),
            connection: Arc::new(Connection::from_stream(connection, identity)?),
            routes,
            resources: Mutex::new(HashMap::new()),
            objects: Mutex::new(HashMap::new()),
            exports: Mutex::new(HashSet::new()),
            callbacks: Arc::clone(&self.callbacks),
        })
    }

    /// Starts an app and serves its host RIDL imports on a runtime worker.
    /// Applications do not need to manually pump IPC while the plugin runs.
    ///
    /// # Errors
    /// Returns an error if the app cannot be resolved or launched.
    pub fn start_managed(&self, catalog: &Catalog, id: &PluginId) -> io::Result<ManagedApp> {
        let app = Arc::new(self.start(catalog, id)?);
        let dispatch = Arc::clone(&app);
        let worker = thread::spawn(move || {
            loop {
                match dispatch.dispatch_next_worker() {
                    Ok(true) => {}
                    Ok(false) => return Ok(()),
                    Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        });
        Ok(ManagedApp {
            app,
            worker: Some(worker),
            callbacks: Arc::clone(&self.callbacks),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ResourceId(pub u32);

/// The recipient receives a separately owned OS descriptor, with the access
/// rights the OS descriptor itself grants. No raw pointers cross the boundary.
pub struct Resource {
    pub descriptor: OwnedFd,
}

/// A resource imported from the plugin. The descriptor is owned locally; its
/// remote reference is released when the last proxy is dropped.
pub struct RemoteResource {
    pub descriptor: OwnedFd,
    pub(crate) _proxy: RemoteObject,
}

/// Called by the Kira runtime inside the already-launched plugin. The socket
/// descriptor came from the parent; there is no discoverable peer address.
pub fn attached_connection() -> io::Result<Option<Connection>> {
    let Some(descriptor) = std::env::var_os(CHANNEL_ENV) else {
        return Ok(None);
    };
    let descriptor = descriptor
        .to_str()
        .and_then(|word| word.parse::<RawFd>().ok())
        .filter(|value| *value >= 0)
        .ok_or_else(|| invalid("invalid inherited RIDL descriptor"))?;
    let identity = std::env::var(ID_ENV)
        .ok()
        .and_then(|word| word.parse::<u64>().ok())
        .ok_or_else(|| invalid("missing inherited RIDL connection identity"))?;
    if CONNECTION_ADOPTED.swap(true, Ordering::AcqRel) {
        return Err(invalid("inherited RIDL connection was already adopted"));
    }
    // SAFETY: the launcher transferred this open descriptor to the child;
    // consuming it here makes this connection responsible for closing it.
    let stream = unsafe { UnixStream::from_raw_fd(descriptor) };
    Connection::from_stream(stream, identity).map(Some)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}
