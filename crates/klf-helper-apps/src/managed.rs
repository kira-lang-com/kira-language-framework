//! Managed Kira helper lifecycle and callback servicing.

use crate::callbacks::Callbacks;
use crate::runtime::{RemoteResource, Resource, ResourceId, Service};
use klf_ridl_wire::transport::{Connection, RemoteObject};
use klf_ridl_wire::{Call, Message, Reply, Value};
use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind};
use std::process::Child;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

fn invalid(message: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

/// A running app. Marketplace plugins import the host's methods only; helpers
/// may additionally export methods that the host calls through `call`.
pub struct HostedApp {
    pub(crate) child: Mutex<Child>,
    pub(crate) connection: Arc<Connection>,
    pub(crate) routes: HashMap<String, Service>,
    pub(crate) resources: Mutex<HashMap<ResourceId, RemoteResource>>,
    pub(crate) objects: Mutex<HashMap<ResourceId, RemoteObject>>,
    pub(crate) exports: Mutex<HashSet<ResourceId>>,
    pub(crate) callbacks: Arc<Callbacks>,
}

impl HostedApp {
    pub fn call(&self, method: &str, arguments: Vec<Value>) -> io::Result<Reply> {
        self.connection.call(method, arguments)
    }

    pub fn share(&self, id: ResourceId, resource: &Resource) -> io::Result<()> {
        let mut exports = self.exports.lock().unwrap_or_else(PoisonError::into_inner);
        if !exports.insert(id) {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                "RIDL resource id is already exported",
            ));
        }
        if let Err(error) = self.connection.export_resource(id.0, &resource.descriptor) {
            exports.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    /// Services one incoming message without exposing commands or socket paths.
    /// A plugin cannot use this connection to call unimported RIDL modules.
    pub fn dispatch_next(&self) -> io::Result<bool> {
        self.dispatch_next_inner(true)
    }

    pub(crate) fn dispatch_next_worker(&self) -> io::Result<bool> {
        self.dispatch_next_inner(false)
    }

    fn dispatch_next_inner(&self, on_host_thread: bool) -> io::Result<bool> {
        let incoming = self.connection.receive()?;
        match incoming.message {
            Message::Request(_) if on_host_thread => self.callbacks.with_owner(|| {
                self.connection
                    .respond(incoming, |call| self.dispatch(call))
            })?,
            Message::Request(_) => self
                .connection
                .respond(incoming, |call| self.dispatch(call))?,
            Message::Exported { object, .. } => {
                let id = ResourceId(object);
                if let Some(descriptor) = incoming.resource {
                    let mut resources = self
                        .resources
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if resources.contains_key(&id) {
                        return Err(invalid("duplicate RIDL resource export"));
                    }
                    resources.insert(
                        id,
                        RemoteResource {
                            descriptor,
                            _proxy: self.connection.proxy(object),
                        },
                    );
                } else {
                    let mut objects = self.objects.lock().unwrap_or_else(PoisonError::into_inner);
                    if objects.contains_key(&id) {
                        return Err(invalid("duplicate RIDL object export"));
                    }
                    objects.insert(id, self.connection.proxy(object));
                }
            }
            Message::Released { object, .. } => {
                if !self
                    .exports
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&ResourceId(object))
                {
                    return Err(invalid("release of an unknown local RIDL export"));
                }
            }
            Message::Closed { .. } => return Ok(false),
            Message::Reply { .. } => return Err(invalid("unexpected RIDL reply on main channel")),
        }
        Ok(true)
    }

    fn dispatch(&self, call: Call) -> Reply {
        match self.routes.get(&call.function) {
            Some(handler) => handler(&call.arguments),
            None => Reply::Refused(format!("RIDL import {} was not granted", call.function)),
        }
    }

    pub fn take_resource(&self, id: ResourceId) -> Option<RemoteResource> {
        self.resources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    pub fn take_object(&self, id: ResourceId) -> Option<RemoteObject> {
        self.objects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    pub fn try_wait(&self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_wait()
    }

    /// Waits for the plugin to exit naturally. Unlike `stop`, this does not
    /// forcibly terminate a child that just sent its RIDL close frame.
    ///
    /// # Errors
    /// Returns OS errors from waiting on the child process.
    pub fn wait(&self) -> io::Result<std::process::ExitStatus> {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wait()
    }

    pub fn stop(&self) -> io::Result<std::process::ExitStatus> {
        let _ = self.connection.close();
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()
    }
}

impl Drop for HostedApp {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(PoisonError::into_inner);
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A running Kira app whose host RIDL requests are automatically dispatched.
/// The event loop, IPC setup and child lifecycle belong to Kira's runtime.
pub struct ManagedApp {
    pub(crate) app: Arc<HostedApp>,
    pub(crate) worker: Option<JoinHandle<io::Result<()>>>,
    pub(crate) callbacks: Arc<Callbacks>,
}

impl ManagedApp {
    /// Returns the child status without consuming the plugin handle.
    ///
    /// # Errors
    /// Returns an OS error if the process status cannot be queried.
    pub fn try_wait(&self) -> io::Result<Option<std::process::ExitStatus>> {
        self.callbacks.pump();
        self.app.try_wait()
    }

    /// Calls a typed helper's exported RIDL function.
    ///
    /// # Errors
    /// Returns transport and protocol errors from the peer.
    pub fn call(&self, method: &str, arguments: Vec<Value>) -> io::Result<Reply> {
        std::thread::scope(|scope| {
            let request = scope.spawn(|| self.app.call(method, arguments));
            while !request.is_finished() {
                if !self.callbacks.pump() {
                    self.callbacks.wait_for_work();
                }
            }
            request
                .join()
                .map_err(|_| io::Error::other("RIDL call worker panicked"))?
        })
    }

    /// Shares a transferable OS resource with this helper only.
    ///
    /// # Errors
    /// Returns a transport error if the descriptor cannot be shared.
    pub fn share(&self, id: ResourceId, resource: &Resource) -> io::Result<()> {
        self.app.share(id, resource)
    }

    /// Takes ownership of an exported resource from the plugin.
    pub fn take_resource(&self, id: ResourceId) -> Option<RemoteResource> {
        self.app.take_resource(id)
    }

    pub fn take_object(&self, id: ResourceId) -> Option<RemoteObject> {
        self.app.take_object(id)
    }

    /// Waits for the plugin and its RIDL event loop to finish naturally.
    ///
    /// # Errors
    /// Returns an OS error or a RIDL dispatch error.
    pub fn wait(mut self) -> io::Result<std::process::ExitStatus> {
        let status = loop {
            self.callbacks.pump();
            if let Some(status) = self.app.try_wait()? {
                break status;
            }
            self.callbacks.wait_for_work();
        };
        self.join_worker()?;
        Ok(status)
    }

    /// Stops the app and reaps the child plus its background RIDL worker.
    ///
    /// # Errors
    /// Returns an OS error or a RIDL dispatch error.
    pub fn stop(mut self) -> io::Result<std::process::ExitStatus> {
        let status = self.app.stop()?;
        self.join_worker()?;
        Ok(status)
    }

    fn join_worker(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            while !worker.is_finished() {
                if !self.callbacks.pump() {
                    self.callbacks.wait_for_work();
                }
            }
            worker
                .join()
                .map_err(|_| io::Error::other("host RIDL dispatcher panicked"))??;
        }
        Ok(())
    }
}

impl Drop for ManagedApp {
    fn drop(&mut self) {
        let _ = self.app.stop();
        let _ = self.join_worker();
    }
}
