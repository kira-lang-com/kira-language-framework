//! A shell hosting a live runtime device through its C entry points: it
//! starts one, a client pushes an app to it, and the shell reads what the
//! device did.
#![allow(unsafe_code)]

use klf_live::{Client, Pushed};
use std::ffi::{CStr, c_char};
use std::time::{Duration, Instant};

// The archive the shell links defines these.
use klf_vm_rt as _;

unsafe extern "C" {
    fn klfvm_live_start(
        address: *const c_char,
        bundle: *const c_char,
        sysroot: *const c_char,
    ) -> i64;
    fn klfvm_live_address(handle: i64) -> *const c_char;
    fn klfvm_live_next_event(handle: i64) -> i32;
    fn klfvm_live_event_detail(handle: i64) -> *const c_char;
    fn klfvm_live_stop(handle: i64);
}

fn app(answer: i64) -> String {
    format!(
        r#"module @Live lang "test" target "aarch64-apple-darwin" stage raw

func private @main() -> i64 engine(runtime) {{
bb0():
  %value = const.int i64, {answer}
  return %value
}}
"#
    )
}

fn string(pointer: *const c_char) -> String {
    // SAFETY: the device returns NUL-terminated strings that live until the
    // next call for it, and this copies one out at once.
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}

/// The device's events, as codes and details, until `last` arrives, waiting
/// at most a few seconds.
fn events_until(handle: i64, last: (i32, &str)) -> Vec<(i32, String)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        // SAFETY: `handle` names a running device.
        let code = unsafe { klfvm_live_next_event(handle) };
        if code == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        // SAFETY: as above.
        let detail = string(unsafe { klfvm_live_event_detail(handle) });
        let done = code == last.0 && detail == last.1;
        events.push((code, detail));
        if done {
            break;
        }
    }
    events
}

#[test]
fn a_shell_hosts_a_device_an_app_is_pushed_to() {
    // SAFETY: every argument is a NUL-terminated string.
    let handle = unsafe {
        klfvm_live_start(
            c"127.0.0.1:0".as_ptr(),
            c"dev.kira.live".as_ptr(),
            c"".as_ptr(),
        )
    };
    assert!(handle > 0, "the device listens");
    // SAFETY: `handle` names the device just started.
    let address = string(unsafe { klfvm_live_address(handle) });

    let mut client = Client::connect(&address).expect("the device accepts");
    let hello = client
        .handshake("dev.kira.live")
        .expect("the device answers");
    assert!(
        hello.layouts.is_empty(),
        "a shell holds no app until one is pushed"
    );
    assert_eq!(
        client.push("Live", &app(4)).expect("pushes"),
        Pushed::Installed(1)
    );
    assert_eq!(
        client.push("Live", &app(5)).expect("pushes"),
        Pushed::Installed(1)
    );

    let events = events_until(handle, (3, "returned 5"));
    let expected = [
        (1, address.as_str()),
        (2, "Live"),
        (3, "returned 4"),
        (2, "Live"),
        (3, "returned 5"),
    ];
    assert_eq!(
        events,
        expected.map(|(code, detail)| (code, detail.to_owned()))
    );

    // SAFETY: `handle` names a running device.
    unsafe { klfvm_live_stop(handle) };
    // SAFETY: a stopped handle names nothing, which answers "".
    assert_eq!(string(unsafe { klfvm_live_address(handle) }), "");
}
