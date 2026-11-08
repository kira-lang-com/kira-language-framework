# Kira helper applications and marketplace plugins — runtime implementation

This crate contains the **Unix runtime foundation** and the Kira-facing
marketplace plugin lifecycle API. Typed helper-method proxies still require
additional RIDL/compiler integration.

## Implemented

The host resolves an installed application ID through `Catalog`; the plugin
does not supply a command, environment, process arguments or IPC endpoint.
The host owns its trusted VM executor via `Catalog::set_vm_runtime`. Native
and hybrid artifacts can be loaded as separately installed executables only
with `Catalog::install_trusted_native`; the ordinary `Catalog::install` refuses
them until platform sandboxing is available. Each
application receives an inherited, private full-duplex RIDL channel. Artifact
paths are resolved before use, and the child inherits neither host environment
variables nor the host's standard streams.

`Host::start_managed` starts the application and services its RIDL imports on
a runtime worker. `HostedApp` retains the lower-level manually dispatched
surface for embedding. Both allow calls in either direction. The installed
plugin's RIDL import metadata determines which host modules it can call; for
VM plugins, only the actual methods in its compiled `ridl.import.entry` map
are routed to it. ABI
compatibility is checked before startup. The host does not need to know an
installed marketplace plugin's exports. `InstalledApp::from_compiled` loads
KLF's existing `.ridlmeta` format. The installed catalog checks VM import
metadata against the generated `.kirb` declarations, including Kira units that
import the same module more than once. The catalog pins the approved `.kirb`
SHA-256 at installation and verifies it again before routing host methods.
A changed import map invalidates approval.

`Host::export_vm` binds a Kira VM's compiled `ridl.module`, `ridl.surface` and
`ridl.entry` records to actual VM functions, validating the canonical RIDL
metadata hash and scalar ABI against the KIR implementation. No per-function
Rust callback is needed. `Host::export_compiled_vm` additionally reads the
provider's `.klbc`, `.kirb` and `.ridlmeta` files, initializes a restricted
service VM, and registers the exported methods. The host-owned VM must remain
available to its RIDL dispatcher. Calls into the **same VM** that is executing
`Plugin.wait()` now use a host-thread callback queue: the RIDL worker enqueues
the call, and `ManagedApp::wait` and `poll` service it on the waiting thread.
`ManagedApp::call` also services nested helper → host → helper calls while it
waits for its own reply. `Instance::try_call` atomically refuses entry when a
different thread owns the VM; a check-then-call race cannot block the worker.
The queue never enters a host VM directly from an IPC worker thread. A host VM
that never reaches a plugin `poll`, `wait` or `call` will still need a
scheduler-driven callback pump; general arbitrary-running Kira applications
are not asynchronously interrupted for callbacks.

### Native and hybrid Kira RIDL imports

An isolated app can declare `let ridlImports: RidlImportMode = .Host` in
`package.kira`. The Kira compiler emits `ridl.remote` metadata so native and
hybrid output code selects a generated remote adapter rather than linking the
provider's local `ridlcc` entry. Kira source keeps its existing
`@Ridl("HostModule")` declarations; no hand-written IPC API is required.

The current Unix native adapter calls `klfrt_ridl_remote_scalar` with the
original `.ridl` contract and method path. The runtime adopts the inherited
private channel and transfers signed/unsigned integers up to 64 bits, Bool,
F32/F64 through `klf-ridl-wire`. Refused or failed calls trap rather than
returning invented values. Borrowed, pointer-bearing, generic, object and
aggregate declarations are rejected until correct ownership-aware adapters
exist. Windows and wasm remote-native adapters are unsupported.

Tests cover Clang compilation of generated LLVM, frontend adapter selection,
and a trusted native child calling the host over the runtime bridge.
`klf check` passes for the modified Kira definitions. The
`corpus/ridl/remote-plugin` fixture has **not** been run as a freshly compiled
native Kira executable against an updated runtime archive; native+VM hybrid
importer composition remains unverified.

Kira's `Foundation` exposes `Plugin.start(installedId)` and a `PluginHandle`
with `poll`, `wait`, `stop` and `shareFile`. These use KLF kernel services, not
`ProcessCommand`; native and VM applications use the same runtime handles.
The host embedder must initialize the installed catalog and approved RIDL
services with `klf_helper_apps::services::initialize(host, catalog)` before
Kira starts plugins. A standalone Kira host cannot register its own RIDL
exports through this API yet.

`shareFile` takes a typed Foundation `FileHandle` and a distinct
`PluginResourceId`. It clones that file's OS descriptor from the Kira file
table, preserving its existing access rights, and transfers ownership of the
clone to exactly one plugin. The host retains its own file handle. Other
resource types need their own typed adapters.

The transport (`klf-ridl-wire::transport`) implements RIDL section 24.7 calls
and replies (including a separate reply channel per call), nested calls,
resource export/release, and connection close. Unix resources are duplicated
across the connection through `SCM_RIGHTS`; transferred descriptors are not
numeric aliases of the sender's handles. Raw pointers, excessive nesting,
invalid connection IDs and oversized messages are rejected.
The sender closes its local copy of each per-call reply channel immediately
after transfer, so a peer that dies before replying causes EOF instead of a
permanently blocked call. A failed Kira callback returns a RIDL refusal without
poisoning the host callback dispatcher.
Object references change their `Here`/`Peer` perspective on receipt, and the
last clone of a remote proxy sends `Released`. Imported OS resources keep their
remote proxy alive until their local owner drops them. An exported resource ID
cannot be reused until the peer releases it.

The decoder enforces nesting limits *before* decoding nested values, and
rejects multiple descriptors in one message while closing the ones that
arrived. The metadata decoder also rejects unsupported versions, impossible
lengths, and trailing bytes.

VM-only helpers run under `Instance::load_restricted`: every foreign import
must name an explicitly granted capability, and native libraries, native
addresses, variadic and raw function-pointer imports are denied. The
`klf-helper-vm` runner loads `.klbc` and its companion `.kirb` generated by
the *same KLF release*; the latter retains Kira's existing `ridl.import` and
`ridl.import.entry` attributes. Currently it binds **scalar-only** RIDL calls,
preserving signedness from the `.ridl` contract. Complex signatures are
refused, not silently reinterpreted as pointers.

## Evidence

`cargo nextest run -p klf-ridl-wire -p klf-helper-apps --offline`: **41 tests
passed**. These include a real VM child executing compiled bytecode through
generated RIDL imports, a separately launched native fixture calling its host,
automatic managed host dispatch, transfer and reading of a real OS descriptor
in another process, nested requests, release of real remote object proxies, and
rejection of malformed or forbidden calls. The host VM suite also verifies
Kira VM plugin-to-Kira VM host calls, compiled host-service loading, generated
signature validation and denial of methods not in a plugin's import map.
New regressions cover a plugin calling its active host VM during a `wait`, a
nested helper → host → helper call, peer death before a reply, and dispatcher
recovery after a callback panic.

`cargo nextest run -p klf-ridl --test meta_suite --offline`: **10 tests passed**.

`cargo nextest run -p klf-vm --test restricted --offline`: **5 tests passed**,
including refusal of cross-thread VM entry while the VM is active.

The Kira plugin service integration test also covers an installed native child
calling host RIDL exports, invalid plugin IDs, lifecycle handle reuse, and a
Foundation-managed read-only file shared by duplicated OS descriptor.

## Not implemented yet

1. Public **Kira-language** registration of host RIDL implementations and
   typed `HelperApp.start<T>()`. Marketplace `Plugin.start(installedId)` now
   exists, but its catalog and host exports are configured by a Rust embedder.
   That embedder can register compiled Kira host services using their generated
   RIDL metadata; registration directly from a running Kira host is missing.
2. Remote adapters for general `.ridl` types: strings, aggregates, `Result`,
   borrowed values, typed object proxy *method dispatch*, traits and typed
   resource handles. Object reference lifecycle works at the transport layer,
   but generated VM calls currently accept only directly representable scalars.
3. Full native/hybrid Kira package execution with generated remote adapters
   linked against an updated installed runtime archive. Scalar codegen and
   the native cross-process bridge are tested, but full hybrid composition
   and non-scalar adapters remain open.
4. OS sandboxing for **native/hybrid** plugin code, signed installed-package
   verification, permissions and per-plugin resource quotas. A separate
   process and private RIDL channel do **not** prevent an unrestricted native
   plugin from communicating via unrelated operating-system facilities. Only
   explicit trusted-native installs are permitted until sandboxing exists.
5. Windows transport and resource handles, cross-release `.kirb` binding
   metadata compatibility, scheduler polling outside plugin lifecycle calls,
   and the RIDL 24.7.3 channel-stack cycle detector (`KLF-RT-0203`). The
   managed runtime currently uses a worker thread and a host-thread callback
   pump while `poll`, `wait` or a typed helper `call` executes.

Until native/hybrid OS sandboxing and package verification are implemented,
**do not treat native marketplace plugins as untrusted code**.

The `languages/kira/corpus/packages/plugins` package passes the Kira frontend
`check` command and builds to a `.klbc` VM image. The complete native driver
could not link against the old
installed `26.10.4` kernel archive: it lacks `klfsvc_plugin_*` and an existing
`klfsvc_source_directory_child` service. A native executable run is therefore
not counted as verified until that archive is updated.