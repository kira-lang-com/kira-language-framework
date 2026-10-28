//! The engine seam a live device is built around (section 17).
//!
//! A device runs a program on some engine. The VM interprets bytecode, the
//! native backend has compiled machine code, and the hybrid backend runs both
//! in one address space. What all three share is the shape below: a device
//! builds what it will run from pushed KIR, installs the first version, swaps
//! a running one for a new version, runs it, and retires the outgoing version
//! once nothing is running it.
//!
//! This is the shared API. The VM's implementation of it is [`VmEngine`]. The
//! native and hybrid bodies are where a device writes a binary, re-signs it
//! with its own identity through a [`Signer`], and launches it as a child
//! process the way a shell would; they are built against these same
//! signatures and do not exist yet.

use crate::artifact::{Artifact, Bundle, RunOutcome, RunRequest};
use crate::transport::Connection;
use crate::wire::{Ack, Frame, Hello, Nack, Push, Reason};
use klf_kir::Module;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a push waits for frames running the version it would retire
/// before the device refuses it as `OLD_CODE_BUSY` (section 17.5).
pub const BUSY_WAIT: Duration = Duration::from_secs(5);
/// How often a waiting push looks again.
const BUSY_POLL: Duration = Duration::from_millis(50);

/// What running a call on the VM produced. A native or hybrid run produces a
/// [`RunOutcome`] instead, which is why that is the type the shared seam uses
/// and this is only what the VM body speaks.
pub type Outcome = klf_vm::Outcome;

/// One engine a device can run on (section 17.1).
///
/// The three engines differ in what a push becomes and in what running it
/// means, so the trait is drawn at the points where they agree: a device builds
/// the pushed KIR into something runnable, installs it beside the version
/// already running, and runs it the way that engine runs anything.
///
/// The VM interprets bytecode inside the device's own process. The native
/// backends write a binary, re-sign it, and launch it as a child process with
/// its own `argv` and `envp`, which is what makes a pushed app behave like a
/// normally launched one. Hybrid holds both halves in one address space
/// (17.2): a native half the bytecode calls into, launched with it.
pub trait Engine {
    /// Which engine this is, as a device names it in a log.
    fn kind(&self) -> &'static str;

    /// Builds `module` into what this engine runs.
    ///
    /// This is the device's own step: a push carries KIR, and turning it into
    /// bytecode or into a binary needs the toolchain the device runs. A native
    /// or hybrid body re-signs the binary it wrote here, so what the device
    /// later launches is what it built and signed, not what it was handed.
    ///
    /// # Errors
    /// Returns why the module cannot be built for this engine.
    fn build(&mut self, module: &Module) -> Result<Artifact, EngineError>;

    /// Installs `artifact` as the first version, running whatever its
    /// initializers do.
    ///
    /// # Errors
    /// Returns why the artifact cannot be installed.
    fn install(&mut self, artifact: &Artifact) -> Result<(), EngineError>;

    /// Replaces the running version with `artifact`, keeping the values a
    /// running program already holds.
    ///
    /// A VM keeps the outgoing frames alive in their own code until they
    /// return. A native or hybrid device keeps the outgoing process alive until
    /// it exits, and the two versions are the two processes; both are the
    /// two-version rule of section 17.5, expressed in what that engine runs.
    ///
    /// # Errors
    /// Returns [`Reason::OldCodeBusy`] when both versions are still running and
    /// no third can be installed, and [`Reason::UnknownModule`] when nothing
    /// is installed to replace.
    fn swap(&mut self, artifact: &Artifact) -> Result<(), Refusal>;

    /// Runs `entry` on the current version as `request`.
    ///
    /// A VM enters `entry` in its own process and `request` has nothing to
    /// change there. A native or hybrid body launches the program with
    /// `request`'s arguments, environment and directory, so its `main` gets
    /// `argc`, `argv` and `envp` from its own process.
    ///
    /// # Errors
    /// Returns why the run could not start.
    fn run(&mut self, entry: &str, request: &RunRequest) -> Result<RunOutcome, EngineError>;

    /// Stops holding the outgoing version once nothing is running it, freeing
    /// its code or letting its process finish. Returns what it retired.
    fn reap(&mut self) -> Vec<Artifact>;
}

/// Re-signs a binary a device built, so the host it runs on will launch it.
///
/// A native or hybrid device does what `LiveContainer` does: the binary it
/// lowered from pushed KIR carries no signature the host trusts, so the device
/// signs it with its own identity before launching it. The signature is the
/// device's, over what the device built; a client never signs anything.
pub trait Signer {
    /// Signs the program at `program` in place.
    ///
    /// # Errors
    /// Returns why the program cannot be signed.
    fn sign(&self, program: &std::path::Path) -> Result<(), EngineError>;
}

/// A push a device refused, with the reason and the device's own words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub reason: Reason,
    pub message: String,
}

impl Refusal {
    #[must_use]
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    fn frame(self, module: &str) -> Frame {
        Frame::Nack(Nack {
            module: module.to_owned(),
            reason: self.reason,
            message: self.message,
        })
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.message)
    }
}

/// Something about the module itself, before it reaches an engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

/// A device: one engine, the modules it holds, and the exchange that pushes
/// new versions into it.
///
/// The device owns the whole section 17.5 loop. A client connects, says hello,
/// and pushes modules; the device answers each with an ack or a nack. Nothing
/// about that is specific to one engine, which is why it lives here and the
/// engine only has to build, install, swap and run.
pub struct Device<E: Engine> {
    engine: E,
    /// The layout each installed module currently has, which is what a hello
    /// reports and a push is checked against.
    layouts: Vec<(String, String)>,
    /// What bundle this device runs. A release bundle accepts no push.
    bundle: Bundle,
}

impl<E: Engine> Device<E> {
    #[must_use]
    pub fn new(engine: E, bundle: Bundle) -> Self {
        Self {
            engine,
            layouts: Vec::new(),
            bundle,
        }
    }

    #[must_use]
    pub fn engine(&self) -> &E {
        &self.engine
    }

    #[must_use]
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// Installs `module` as the first version of the app this device runs,
    /// which is how a device starts: a push only replaces a module the device
    /// already holds (section 17.5, `UNKNOWN_MODULE`).
    ///
    /// # Errors
    /// Returns why the module cannot be built or installed.
    pub fn install(&mut self, module: &Module) -> Result<(), EngineError> {
        let artifact = self.engine.build(module)?;
        self.engine.install(&artifact)?;
        self.remember(module.name.as_str(), &crate::wire::layout_hash(module));
        Ok(())
    }

    /// The hello this device answers a client's hello with: its own version,
    /// its bundle and the layout hash of every module it holds.
    #[must_use]
    pub fn hello(&self, klf_version: &str) -> Hello {
        let mut layouts = self.layouts.clone();
        layouts.sort();
        Hello {
            version: crate::wire::VERSION,
            klf_version: klf_version.to_owned(),
            bundle: self.bundle.id.clone(),
            layouts,
        }
    }

    /// The device's answer to one frame.
    #[must_use]
    pub fn on_frame(&mut self, frame: &Frame, klf_version: &str) -> Frame {
        match frame {
            Frame::Push(push) => match self.on_push(push) {
                Ok(ack) => Frame::Ack(ack),
                Err(refusal) => refusal.frame(&push.module),
            },
            Frame::Hello(_) => Frame::Hello(self.hello(klf_version)),
            other => other.clone(),
        }
    }

    /// A release device refuses every push (section 17.5: hot push is
    /// available only in debug bundles), so the check comes before anything is
    /// parsed.
    fn on_push(&mut self, push: &Push) -> Result<Ack, Refusal> {
        if !self.bundle.accepts_pushes() {
            return Err(Refusal::new(
                Reason::VerifyFailed,
                "hot push is available only in a debug bundle",
            ));
        }
        let module = klf_kir::parse_module(&push.kir).map_err(|error| {
            Refusal::new(
                Reason::VerifyFailed,
                format!("the pushed KIR does not parse: {error:?}"),
            )
        })?;
        let hash = crate::wire::layout_hash(&module);
        let replaced = u32::try_from(module.functions.len()).unwrap_or(u32::MAX);
        match self.layouts.iter().find(|(name, _)| name == &push.module) {
            // A debug device holding nothing installs the first module pushed
            // to it (section 17.5): a shell ships with no app.
            None if self.layouts.is_empty() => {
                self.install(&module).map_err(|error| {
                    Refusal::new(
                        Reason::VerifyFailed,
                        format!("the device cannot install this module: {error}"),
                    )
                })?;
                return Ok(Ack {
                    module: push.module.clone(),
                    replaced,
                });
            }
            None => {
                return Err(Refusal::new(
                    Reason::UnknownModule,
                    format!("this device does not hold `{}`", push.module),
                ));
            }
            Some((_, current)) if current != &hash => {
                return Err(Refusal::new(
                    Reason::LayoutChanged,
                    "the layout differs from the loaded module; restart the device",
                ));
            }
            Some(_) => {}
        }
        let artifact = self.engine.build(&module).map_err(|error| {
            Refusal::new(
                Reason::VerifyFailed,
                format!("the device cannot build this module: {error}"),
            )
        })?;
        self.swap_waiting(&artifact)?;
        self.engine.reap();
        self.remember(&push.module, &hash);
        Ok(Ack {
            module: push.module.clone(),
            replaced,
        })
    }

    /// Swaps `artifact` in, waiting up to [`BUSY_WAIT`] for frames still
    /// running the version it would retire to return (section 17.5) before
    /// refusing with `OLD_CODE_BUSY`.
    fn swap_waiting(&mut self, artifact: &Artifact) -> Result<(), Refusal> {
        let deadline = Instant::now() + BUSY_WAIT;
        loop {
            match self.engine.swap(artifact) {
                Err(refusal)
                    if refusal.reason == Reason::OldCodeBusy && Instant::now() < deadline =>
                {
                    std::thread::sleep(BUSY_POLL);
                    self.engine.reap();
                }
                outcome => return outcome,
            }
        }
    }

    /// Answers every frame a client sends over `connection` until it hangs
    /// up. Each answer to a push goes to `landed` too, which is where a host
    /// reruns the app or shows what the device said.
    ///
    /// # Errors
    /// Returns a message when a frame cannot be read or written.
    pub fn answer<S: std::io::Read + std::io::Write>(
        &mut self,
        connection: &mut Connection<S>,
        klf_version: &str,
        landed: &mut dyn FnMut(&mut Self, &Frame),
    ) -> Result<(), String> {
        loop {
            let frame = match connection.read() {
                Ok(frame) => frame,
                Err(message) if message.contains("cannot read a frame header") => return Ok(()),
                Err(message) => return Err(message),
            };
            let answer = self.on_frame(&frame, klf_version);
            connection.write(&answer)?;
            if let Frame::Push(_) = frame {
                landed(self, &answer);
            }
        }
    }

    /// Serves the clients that connect to `listener`, one after another, for
    /// as long as it accepts. A device listening on a tailnet address is
    /// reached from any machine on the tailnet. A client that breaks its
    /// connection ends only that connection, and `dropped` hears why.
    pub fn serve(
        &mut self,
        listener: &std::net::TcpListener,
        klf_version: &str,
        landed: &mut dyn FnMut(&mut Self, &Frame),
        dropped: &mut dyn FnMut(&str),
    ) {
        for stream in listener.incoming() {
            let result = stream
                .map_err(|error| format!("cannot accept a client: {error}"))
                .and_then(|stream| self.answer(&mut Connection::new(stream), klf_version, landed));
            if let Err(message) = result {
                dropped(&message);
            }
        }
    }

    /// Records `module`'s layout, or replaces what it held.
    fn remember(&mut self, module: &str, hash: &str) {
        match self.layouts.iter_mut().find(|(name, _)| name == module) {
            Some(entry) => hash.clone_into(&mut entry.1),
            None => self.layouts.push((module.to_owned(), hash.to_owned())),
        }
    }
}

/// The VM's implementation of [`Engine`]: it holds a `klf-vm` instance and
/// installs a pushed module by lowering its KIR to bytecode and adopting it as
/// the next code version.
pub struct VmEngine {
    instance: Option<Box<klf_vm::Instance>>,
    /// The version new calls enter.
    current: Option<Artifact>,
    /// The version a swap replaced, held until no frame is running it.
    outgoing: Option<Artifact>,
    /// Versions the VM has dropped and `reap` has not yet reported.
    retired: Vec<Artifact>,
    base: std::path::PathBuf,
    /// The sysroot whose std bytecode a pushed app is linked against, when the
    /// device carries one.
    sysroot: Option<std::path::PathBuf>,
}

impl VmEngine {
    /// A device that has installed nothing yet. `base` is where the native
    /// libraries a module imports sit.
    #[must_use]
    pub fn new(base: impl Into<std::path::PathBuf>) -> Self {
        Self {
            instance: None,
            current: None,
            outgoing: None,
            retired: Vec::new(),
            base: base.into(),
            sysroot: None,
        }
    }

    /// Links every app this device builds against the std bytecode `root`
    /// carries for the app's language (section 5.4), which is what a pushed
    /// package's KIR calls into: the client sends the package, the device
    /// supplies the std it already has.
    #[must_use]
    pub fn with_sysroot(mut self, root: impl Into<std::path::PathBuf>) -> Self {
        self.sysroot = Some(root.into());
        self
    }

    /// The program `artifact` holds, decoded from the bytes the device built
    /// and named, so what runs is exactly what the digest names.
    fn program(artifact: &Artifact) -> Result<klf_vm::Program, String> {
        match artifact {
            Artifact::Bytecode { image, .. } => klf_vm::decode(image)
                .map_err(|error| format!("the bytecode does not decode: {error}")),
            other => Err(format!(
                "a VM device cannot run a {} artifact",
                other.kind()
            )),
        }
    }
}

impl Engine for VmEngine {
    fn kind(&self) -> &'static str {
        "vm"
    }

    fn build(&mut self, module: &Module) -> Result<Artifact, EngineError> {
        let mut program = klf_vm::lower_module(module)
            .map_err(|error| EngineError(format!("cannot lower to bytecode: {error}")))?;
        if let Some(root) = &self.sysroot {
            let path =
                klf_vm::std_bytecode_path(root, &module.language, module.target.pointer_width());
            klf_vm::link_std(&mut program, &path).map_err(EngineError)?;
        }
        let entries = module
            .functions
            .iter()
            .filter(|function| klf_vm::entered_from_c(function))
            .map(|function| {
                (
                    function.name.as_str().to_owned(),
                    klf_vm::entry_signature(function),
                )
            })
            .collect();
        Ok(Artifact::Bytecode {
            image: klf_vm::encode(&program),
            entries,
        })
    }

    fn install(&mut self, artifact: &Artifact) -> Result<(), EngineError> {
        let program = Self::program(artifact).map_err(EngineError)?;
        let instance = klf_vm::Instance::load(program, &self.base, klf_vm::kernel_services().0)
            .map_err(EngineError)?;
        if let Artifact::Bytecode { entries, .. } = artifact {
            let unbound = instance.provide_entries(entries);
            if !unbound.is_empty() {
                return Err(EngineError(format!(
                    "no native entry for {}, which C calls",
                    unbound.join(", ")
                )));
            }
        }
        match instance.initialize() {
            Outcome::Return(_) => {
                self.instance = Some(instance);
                self.current = Some(artifact.clone());
                Ok(())
            }
            Outcome::Fail(value) => Err(EngineError(format!("initializers failed with {value}"))),
            Outcome::Trap(code) => Err(EngineError(format!("initializers trapped: {code}"))),
        }
    }

    fn swap(&mut self, artifact: &Artifact) -> Result<(), Refusal> {
        let instance = self
            .instance
            .as_deref()
            .ok_or_else(|| Refusal::new(Reason::UnknownModule, "no module is installed"))?;
        // The native half calls what the running version's entries name, with
        // the signatures they were bound with.
        if let (
            Some(Artifact::Bytecode {
                entries: running, ..
            }),
            Artifact::Bytecode {
                entries: pushed, ..
            },
        ) = (&self.current, artifact)
            && let Some((name, _)) = running.iter().find(|entry| !pushed.contains(entry))
        {
            return Err(Refusal::new(
                Reason::SignatureChanged,
                format!("`{name}`, which the native half calls, changed signature or is gone"),
            ));
        }
        let program = Self::program(artifact)
            .and_then(|program| klf_vm::rebase_imports(program, instance.program()))
            .map_err(|message| Refusal::new(Reason::VerifyFailed, message))?;
        instance
            .adopt(Arc::new(klf_vm::Code::new(program)))
            .map_err(|busy| Refusal::new(Reason::OldCodeBusy, busy.to_string()))?;
        instance.retarget_entries();
        // The adopt overwrote the slot the older outgoing version held.
        self.retired.extend(self.outgoing.take());
        self.outgoing = self.current.replace(artifact.clone());
        Ok(())
    }

    fn run(&mut self, entry: &str, _request: &RunRequest) -> Result<RunOutcome, EngineError> {
        let instance = self
            .instance
            .as_deref()
            .ok_or_else(|| EngineError("no module is installed".to_owned()))?;
        let index = instance
            .function_index(entry)
            .ok_or_else(|| EngineError(format!("no function named `{entry}`")))?;
        Ok(match instance.call(index, &[]) {
            Outcome::Return(value) => RunOutcome::Returned(value.unwrap_or(0)),
            Outcome::Fail(value) => RunOutcome::Failed(format!("failed with {value}")),
            Outcome::Trap(code) => RunOutcome::Failed(code),
        })
    }

    fn reap(&mut self) -> Vec<Artifact> {
        if let Some(instance) = self.instance.as_deref() {
            instance.reap();
            let previous = 1 - instance.version();
            if !instance.version_busy(previous) {
                self.retired.extend(self.outgoing.take());
            }
        }
        std::mem::take(&mut self.retired)
    }
}
