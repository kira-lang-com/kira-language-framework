//! A live runtime device behind C entry points (section 17.5), so a shell
//! written in any KLF language hosts one in its own process: it starts a
//! device listening on an address, reads what the device did, and stops it. The device is a debug one, since a live runtime exists
//! to take pushes, and it holds no app until the first push installs one.
//!
//! A device is named by a handle, a positive integer, so a shell passes no
//! pointer across. What the device did comes back one event at a time, as a
//! code and a line of detail. Strings cross as NUL-terminated UTF-8; one the
//! device returns stays valid until the shell's next call for that device.

use klf_live::wire::Frame;
use klf_live::{Bundle, Device, Engine, RunRequest, VmEngine};
use std::ffi::{CStr, CString, c_char};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

/// The function a device runs each time an app lands.
const ENTRY: &str = "main";

/// What a device did, as `klfvm_live_next_event` codes it. The detail of
/// each is the text `klfvm_live_event_detail` returns.
#[derive(Clone, Copy)]
#[repr(i32)]
enum Event {
    /// Nothing since the last call; the detail is "".
    None = 0,
    /// The device listens; the detail is its address.
    Listening = 1,
    /// A push landed; the detail is the module's name.
    Installed = 2,
    /// The app ran after landing; the detail says how it ended.
    Ran = 3,
    /// A push was refused; the detail is the module and why.
    Refused = 4,
    /// A client's connection broke; the detail is why.
    Dropped = 5,
}

/// A running device as the shell sees it.
struct Host {
    /// The address the device listens on, with the port it was given.
    address: CString,
    events: Receiver<(Event, String)>,
    stopping: Arc<AtomicBool>,
    /// The detail of the last event handed to the shell, kept alive for it to
    /// read.
    detail: CString,
}

static HOSTS: Mutex<Vec<Option<Host>>> = Mutex::new(Vec::new());

/// Starts a device listening on `address` (`host:port`; port 0 lets the
/// system choose, and a tailnet address is reached from the whole tailnet).
/// `bundle` is the id its hello reports; `sysroot`, when not empty, is where
/// it finds the std bytecode a pushed app links. Returns the device's handle,
/// or 0 when it cannot listen there.
///
/// # Safety
/// Each argument must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_live_start(
    address: *const c_char,
    bundle: *const c_char,
    sysroot: *const c_char,
) -> i64 {
    // SAFETY: the caller promises NUL-terminated strings or null.
    let (address, bundle, sysroot) = unsafe { (text(address), text(bundle), text(sysroot)) };
    let Ok(listener) = TcpListener::bind(&address) else {
        return 0;
    };
    let Ok(bound) = listener.local_addr() else {
        return 0;
    };
    let Ok(shown) = CString::new(bound.to_string()) else {
        return 0;
    };
    let (sender, events) = channel();
    let stopping = Arc::new(AtomicBool::new(false));
    let sysroot = (!sysroot.is_empty()).then(|| PathBuf::from(sysroot));
    let flag = Arc::clone(&stopping);
    let started = std::thread::Builder::new()
        .name("klf-live-device".to_owned())
        .spawn(move || serve(&listener, bundle, sysroot, &sender, &flag));
    if started.is_err() {
        return 0;
    }
    let mut hosts = HOSTS.lock().unwrap_or_else(PoisonError::into_inner);
    hosts.push(Some(Host {
        address: shown,
        events,
        stopping,
        detail: CString::default(),
    }));
    i64::try_from(hosts.len()).unwrap_or(0)
}

/// The address device `handle` listens on, or "" when there is no such
/// device.
#[unsafe(no_mangle)]
pub extern "C" fn klfvm_live_address(handle: i64) -> *const c_char {
    with_host(handle, |host| host.address.as_ptr())
}

/// The next thing device `handle` did, as an [`Event`] code: 0 when it has
/// done nothing since the last call or there is no such device.
#[unsafe(no_mangle)]
pub extern "C" fn klfvm_live_next_event(handle: i64) -> i32 {
    let mut code = Event::None as i32;
    with_host(handle, |host| {
        let (event, detail) = host
            .events
            .try_recv()
            .unwrap_or((Event::None, String::new()));
        host.detail = CString::new(detail).unwrap_or_default();
        code = event as i32;
        host.detail.as_ptr()
    });
    code
}

/// The detail of the event `klfvm_live_next_event` last returned for device
/// `handle`, or "" when there is none.
#[unsafe(no_mangle)]
pub extern "C" fn klfvm_live_event_detail(handle: i64) -> *const c_char {
    with_host(handle, |host| host.detail.as_ptr())
}

/// Stops device `handle`: it takes no new client, and the app it runs goes
/// once the client it is answering hangs up.
#[unsafe(no_mangle)]
pub extern "C" fn klfvm_live_stop(handle: i64) {
    let host = usize::try_from(handle)
        .ok()
        .and_then(|handle| handle.checked_sub(1))
        .and_then(|index| {
            HOSTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(index)
                .and_then(Option::take)
        });
    let Some(host) = host else {
        return;
    };
    host.stopping.store(true, Ordering::Release);
    // Wake the accept the device is blocked in, so it sees the flag.
    if let Some(address) = host
        .address
        .to_str()
        .ok()
        .and_then(|address| address.parse::<SocketAddr>().ok())
    {
        let _ = TcpStream::connect(reachable(address));
    }
}

/// Runs `body` on device `handle`, or answers "" when there is none.
fn with_host(handle: i64, body: impl FnOnce(&mut Host) -> *const c_char) -> *const c_char {
    let mut hosts = HOSTS.lock().unwrap_or_else(PoisonError::into_inner);
    let host = usize::try_from(handle)
        .ok()
        .and_then(|handle| handle.checked_sub(1))
        .and_then(|index| hosts.get_mut(index))
        .and_then(Option::as_mut);
    match host {
        Some(host) => body(host),
        None => c"".as_ptr(),
    }
}

/// The address a connection to a device listening on `address` uses: the
/// loopback one when it listens on every interface.
fn reachable(mut address: SocketAddr) -> SocketAddr {
    if address.ip().is_unspecified() {
        match address {
            SocketAddr::V4(_) => address.set_ip(std::net::Ipv4Addr::LOCALHOST.into()),
            SocketAddr::V6(_) => address.set_ip(std::net::Ipv6Addr::LOCALHOST.into()),
        }
    }
    address
}

/// The device's own thread: it answers clients one after another until it
/// is stopped, and reports what it did on `events`.
fn serve(
    listener: &TcpListener,
    bundle: String,
    sysroot: Option<PathBuf>,
    events: &Sender<(Event, String)>,
    stopping: &AtomicBool,
) {
    let base = sysroot.clone().unwrap_or_else(std::env::temp_dir);
    let engine = match sysroot {
        Some(root) => VmEngine::new(base).with_sysroot(root),
        None => VmEngine::new(base),
    };
    let mut device = Device::new(
        engine,
        Bundle {
            id: bundle,
            debug: true,
        },
    );
    let request = RunRequest::new();
    if let Ok(address) = listener.local_addr() {
        let _ = events.send((Event::Listening, address.to_string()));
    }
    for stream in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            return;
        }
        let result = stream
            .map_err(|error| format!("cannot accept a client: {error}"))
            .and_then(|stream| {
                device.answer(
                    &mut klf_live::Connection::new(stream),
                    env!("CARGO_PKG_VERSION"),
                    &mut |device, answer| {
                        landed(device, answer, &request, events);
                    },
                )
            });
        if let Err(message) = result {
            let _ = events.send((Event::Dropped, message));
        }
    }
}

/// Reports a push's answer, and runs the app again when it landed.
fn landed(
    device: &mut Device<VmEngine>,
    answer: &Frame,
    request: &RunRequest,
    events: &Sender<(Event, String)>,
) {
    match answer {
        Frame::Ack(ack) => {
            let _ = events.send((Event::Installed, ack.module.clone()));
            let ran = match device.engine_mut().run(ENTRY, request) {
                Ok(outcome) => outcome.to_string(),
                Err(error) => format!("did not run: {error}"),
            };
            let _ = events.send((Event::Ran, ran));
        }
        Frame::Nack(nack) => {
            let _ = events.send((Event::Refused, format!("{}: {}", nack.module, nack.message)));
        }
        // A push is answered with an acknowledgement or a refusal only.
        _ => {}
    }
}

/// The string at `pointer`, or "" when it is null.
///
/// # Safety
/// `pointer` must be null or a NUL-terminated string.
unsafe fn text(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: the caller promises a NUL-terminated string.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}
