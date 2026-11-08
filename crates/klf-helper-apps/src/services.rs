//! Runtime-owned handles for Kira's `Plugin` service API.
//!
//! The embedder supplies installed apps and host RIDL exports once, before
//! Kira code starts plugins. No plugin can supply a command or IPC address.

use crate::{Catalog, Host, ManagedApp, PluginId, Resource, ResourceId};
use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, OnceLock, PoisonError};

pub const NOT_CONFIGURED: i64 = -1;
pub const NOT_INSTALLED: i64 = -2;
pub const START_FAILED: i64 = -3;
pub const INVALID_HANDLE: i64 = -4;
pub const WAIT_FAILED: i64 = -5;
pub const INVALID_RESOURCE: i64 = -6;
pub const ALREADY_SHARED: i64 = -7;
pub const STILL_RUNNING: i64 = -100;

struct Running {
    app: ManagedApp,
}

struct Runtime {
    host: Host,
    catalog: Catalog,
    next_handle: i64,
    running: HashMap<i64, Running>,
}

static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

/// Installs the host's approved RIDL exports and installed application catalog.
/// This must be called once before the host Kira application runs.
///
/// # Errors
/// Returns AlreadyExists if the Kira host runtime was already initialized.
pub fn initialize(host: Host, catalog: Catalog) -> io::Result<()> {
    RUNTIME
        .set(Mutex::new(Runtime {
            host,
            catalog,
            next_handle: 1,
            running: HashMap::new(),
        }))
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Kira plugin runtime is already configured",
            )
        })
}

/// Starts an installed plugin by its canonical ID and returns an opaque handle.
#[must_use]
pub fn start(id: &str) -> i64 {
    let Ok(id) = PluginId::new(id) else {
        return NOT_INSTALLED;
    };
    let Some(state) = RUNTIME.get() else {
        return NOT_CONFIGURED;
    };
    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.catalog.get(&id).is_none() {
        return NOT_INSTALLED;
    }
    let Ok(app) = state.host.start_managed(&state.catalog, &id) else {
        return START_FAILED;
    };
    let handle = state.next_handle;
    let Some(next) = handle.checked_add(1) else {
        return START_FAILED;
    };
    state.next_handle = next;
    state.running.insert(handle, Running { app });
    handle
}

/// Returns STILL_RUNNING, the exit status, or a failure. An observed exit is
/// not consumed: `wait` must still release the runtime handle.
#[must_use]
pub fn poll(handle: i64) -> i64 {
    let Some(state) = RUNTIME.get() else {
        return NOT_CONFIGURED;
    };
    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(running) = state.running.get(&handle) else {
        return INVALID_HANDLE;
    };
    running.app.try_wait().map_or(WAIT_FAILED, |status| {
        status.map_or(STILL_RUNNING, exit_word)
    })
}

/// Waits for the plugin's process and RIDL dispatcher to finish, then consumes
/// its handle. Does not run a shell or expose any child-process machinery.
#[must_use]
pub fn wait(handle: i64) -> i64 {
    finish(handle, ManagedApp::wait)
}

/// Stops the plugin and its dispatcher, consuming the runtime handle.
#[must_use]
pub fn stop(handle: i64) -> i64 {
    finish(handle, ManagedApp::stop)
}

/// Duplicates a Kira file into a plugin, retaining the file's original rights.
/// A plugin never sees the source process's numeric file-handle identifier.
#[must_use]
pub fn share_file(handle: i64, resource_id: i64, file_handle: i64) -> i64 {
    let Some(resource_id) = u32::try_from(resource_id).ok().filter(|id| *id != 0) else {
        return INVALID_RESOURCE;
    };
    let Some(state) = RUNTIME.get() else {
        return NOT_CONFIGURED;
    };
    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(running) = state.running.get(&handle) else {
        return INVALID_HANDLE;
    };
    let Ok(descriptor) = klf_host_files::handles::duplicate_descriptor(file_handle) else {
        return INVALID_RESOURCE;
    };
    match running
        .app
        .share(ResourceId(resource_id), &Resource { descriptor })
    {
        Ok(()) => 1,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => ALREADY_SHARED,
        Err(_) => WAIT_FAILED,
    }
}

fn finish(
    handle: i64,
    operation: impl FnOnce(ManagedApp) -> io::Result<std::process::ExitStatus>,
) -> i64 {
    let Some(state) = RUNTIME.get() else {
        return NOT_CONFIGURED;
    };
    let running = state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .running
        .remove(&handle);
    let Some(running) = running else {
        return INVALID_HANDLE;
    };
    operation(running.app).map_or(WAIT_FAILED, exit_word)
}

fn exit_word(status: std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return i64::from(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -200 - i64::from(signal);
        }
    }
    WAIT_FAILED
}
