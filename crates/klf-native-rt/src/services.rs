//! The service table a natively compiled frontend installs around a phase call.
//!
//! On the VM the kernel hands a definition a `Capabilities` table built fresh
//! for each phase. Natively the definition's calls are plain linked symbols, so
//! the equivalent table lives here: the frontend driver installs one before it
//! enters a phase and takes it back out afterwards, and the `klfsvc_*` shims in
//! `service_abi` look their handler up in it. The same `klf-defsvc` bridge
//! builds the table in both cases, so there is one implementation of the
//! `klf.kir.*` builder, not two.
//!
//! The current source text and the diagnostics a phase reported ride along in
//! the same place, for the same reason: `klfsvc_source_byte` and
//! `klfsvc_diag_error` need per-phase context that a bare symbol cannot carry.

use klf_defsvc::Handler;
use klf_diag::RuntimeDiagnostic;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// The error sentinel a service answers with when nothing is installed.
const UNSERVICED: i64 = i64::MAX;

struct Installed {
    /// The handler of each `service_abi` shim, at its `SYMBOLS` index.
    handlers: Vec<Option<Handler>>,
    source: Arc<[u8]>,
    diagnostics: RefCell<Vec<RuntimeDiagnostic>>,
    diagnostic_holds: RefCell<Vec<usize>>,
}

thread_local! {
    static CURRENT: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Makes `table` and `source` the context of the phase about to run.
pub fn install(table: HashMap<String, Handler>, source: String) {
    CURRENT.with(|cell| {
        *cell.borrow_mut() = Some(Installed {
            handlers: crate::service_abi::SYMBOLS
                .iter()
                .map(|symbol| table.get(*symbol).cloned())
                .collect(),
            source: Arc::from(source.into_bytes()),
            diagnostics: RefCell::new(Vec::new()),
            diagnostic_holds: RefCell::new(Vec::new()),
        });
    });
}

/// Ends the phase and returns the diagnostics it reported.
pub fn uninstall() -> Vec<RuntimeDiagnostic> {
    CURRENT.with(|cell| {
        cell.borrow_mut()
            .take()
            .map(|installed| installed.diagnostics.into_inner())
            .unwrap_or_default()
    })
}

/// Whether a frontend phase is currently driving.
pub fn active() -> bool {
    CURRENT.with(|cell| cell.borrow().is_some())
}

/// Calls the installed handler of the shim at `service` in `SYMBOLS`.
///
/// A pending cancellation of the running analysis stops the definition here,
/// before the service runs. The handler runs under a shared borrow of the table, which only install
/// and uninstall take exclusively, between phases. Handlers are closures over
/// the bridge state, which takes its own lock.
pub fn dispatch(service: usize, arguments: &[u64]) -> i64 {
    crate::interrupt::poll_service();
    CURRENT.with(|cell| {
        let installed = cell.borrow();
        match installed
            .as_ref()
            .and_then(|installed| installed.handlers[service].as_ref())
        {
            Some(handler) => crate::with(|rt| {
                handler.call_with_memory(arguments, &mut NativeMemory(&mut rt.heap)) as i64
            }),
            None => UNSERVICED,
        }
    })
}

/// The memory a service reads and writes: any memory native code could
/// reach itself, as the `klfsvc_fs_*` services read it. That covers a
/// frontend's string literals, which live in the executable, and the stack
/// slots its functions pass. A heap address still has to name one complete
/// block, which catches a buffer that runs past its allocation.
struct NativeMemory<'a>(&'a mut klf_rt::Heap);

impl NativeMemory<'_> {
    fn direct(&self, address: u64, length: usize) -> Option<usize> {
        let pointer = usize::try_from(address).ok()?;
        let outside = length > 0 && !self.0.contains(pointer as *const u8);
        (outside && pointer != 0 && length <= klf_rt::MAX_SERVICE_BUFFER_BYTES).then_some(pointer)
    }
}

impl klf_rt::ServiceMemory for NativeMemory<'_> {
    fn read_buffer(&self, address: u64, destination: &mut [u8]) -> bool {
        let Some(pointer) = self.direct(address, destination.len()) else {
            return klf_rt::ServiceMemory::read_buffer(&*self.0, address, destination);
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                pointer as *const u8,
                destination.as_mut_ptr(),
                destination.len(),
            )
        };
        true
    }

    fn write_buffer(&mut self, address: u64, source: &[u8]) -> bool {
        let Some(pointer) = self.direct(address, source.len()) else {
            return klf_rt::ServiceMemory::write_buffer(self.0, address, source);
        };
        unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), pointer as *mut u8, source.len()) };
        true
    }
}

/// Hands `read` the bytes `klfsvc_source_len`, `klfsvc_source_byte` and
/// `klfsvc_source_copy` read.
///
/// With no phase installed this falls back to `KLF_SOURCE`, which is how the
/// driver's plain native test harness feeds a program its input.
pub fn with_source<R>(read: impl FnOnce(&[u8]) -> R) -> R {
    let installed = CURRENT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|installed| Arc::clone(&installed.source))
    });
    match installed {
        Some(source) => read(&source),
        None => read(std::env::var("KLF_SOURCE").unwrap_or_default().as_bytes()),
    }
}

/// Records a diagnostic the running phase reported. Returns false when no phase
/// is driving, in which case the caller prints it instead.
pub fn record_diagnostic(record: RuntimeDiagnostic) -> bool {
    CURRENT.with(|cell| {
        let borrowed = cell.borrow();
        let Some(installed) = borrowed.as_ref() else {
            return false;
        };
        installed.diagnostics.borrow_mut().push(record);
        true
    })
}

/// Attaches `detail` to the diagnostic the running phase reported last.
/// Returns false when no phase is driving, in which case the caller prints it
/// instead; true, doing nothing, when the phase has reported none.
pub fn record_detail(detail: klf_diag::RuntimeDetail) -> bool {
    CURRENT.with(|cell| {
        let borrowed = cell.borrow();
        let Some(installed) = borrowed.as_ref() else {
            return false;
        };
        if let Some(last) = installed.diagnostics.borrow_mut().last_mut() {
            last.details.push(detail);
        }
        true
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_hold() {
    CURRENT.with(|cell| {
        if let Some(installed) = cell.borrow().as_ref() {
            installed
                .diagnostic_holds
                .borrow_mut()
                .push(installed.diagnostics.borrow().len());
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_release(keep: i64) -> i64 {
    CURRENT.with(|cell| {
        let current = cell.borrow();
        let Some(installed) = current.as_ref() else {
            return 0;
        };
        let Some(start) = installed.diagnostic_holds.borrow_mut().pop() else {
            return 0;
        };
        let mut diagnostics = installed.diagnostics.borrow_mut();
        let count = diagnostics.len().saturating_sub(start);
        if keep == 0 {
            diagnostics.truncate(start);
        }
        i64::try_from(count).unwrap_or(i64::MAX)
    })
}
