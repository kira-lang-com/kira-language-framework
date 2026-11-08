//! Kira helper applications and installed plugins.
//!
//! Launching is a runtime operation, not a Kira `ProcessCommand`. A helper
//! has a known RIDL surface; a marketplace plugin only imports host RIDL.
//! The two use identical private process connections and resource transfers.

#[cfg(unix)]
mod callbacks;
#[cfg(unix)]
mod host_vm;
#[cfg(unix)]
mod managed;
#[cfg(unix)]
mod runtime;
#[cfg(unix)]
pub mod services;
#[cfg(unix)]
mod vm;

#[cfg(unix)]
pub use managed::{HostedApp, ManagedApp};
#[cfg(unix)]
pub use runtime::{
    Artifact, Catalog, Host, InstalledApp, PluginId, RemoteResource, Resource, ResourceId, Service,
    attached_connection,
};
#[cfg(unix)]
pub use vm::bind_generated_ridl_imports;
#[cfg(unix)]
pub use vm::{ScalarImport, bind_vm_host_imports};
