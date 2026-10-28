//! The section 17.5 exchange, end to end: a client pushes KIR over the wire, a
//! device lowers it, installs it, and answers with an ack or a nack.

use std::fmt::Write as _;

use klf_live::wire::{Ack, Frame, Hello, Nack, Push, Reason};
use klf_live::{Bundle, Device, Engine, RunOutcome, RunRequest, VmEngine};

fn module(answer: i64) -> String {
    format!(
        r#"module @Live lang "test" target "aarch64-apple-darwin" stage raw

func private @answer() -> i64 engine(runtime) {{
bb0():
  %value = const.int i64, {answer}
  return %value
}}

func private @main() -> i64 engine(runtime) {{
bb0():
  %value = call @answer()
  return %value
}}
"#
    )
}

fn parsed(kir: &str) -> klf_kir::Module {
    klf_kir::parse_module(kir).expect("module parses")
}

fn bundle(debug: bool) -> Bundle {
    Bundle {
        id: "bundle".to_owned(),
        debug,
    }
}

/// A device started the way a device starts: with the app's first version
/// installed, which is what a push then replaces.
fn device(debug: bool) -> Device<VmEngine> {
    let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle(debug));
    device.install(&parsed(&module(1))).expect("installs");
    device
}

fn push(device: &mut Device<VmEngine>, name: &str, kir: String) -> Frame {
    device.on_frame(
        &Frame::Push(Push {
            module: name.to_owned(),
            kir,
        }),
        "test",
    )
}

fn main_of(device: &mut Device<VmEngine>) -> RunOutcome {
    device
        .engine_mut()
        .run("main", &RunRequest::new())
        .expect("runs")
}

#[test]
fn a_pushed_module_is_acknowledged_and_is_what_the_next_call_runs() {
    let mut device = device(true);
    assert_eq!(main_of(&mut device), RunOutcome::Returned(1));

    match push(&mut device, "Live", module(7)) {
        Frame::Ack(Ack { replaced, .. }) => assert_eq!(replaced, 2, "both functions"),
        other => panic!("expected an ack, got {other:?}"),
    }

    assert_eq!(
        main_of(&mut device),
        RunOutcome::Returned(7),
        "the next call enters the code the push installed"
    );
}

#[test]
fn a_push_naming_a_module_the_device_does_not_hold_is_refused() {
    let mut device = device(true);
    match push(&mut device, "Other", module(2)) {
        Frame::Nack(Nack { reason, .. }) => assert_eq!(reason, Reason::UnknownModule),
        other => panic!("expected a nack, got {other:?}"),
    }
}

#[test]
fn the_outgoing_version_is_retired_once_nothing_runs_it() {
    let mut engine = VmEngine::new(std::env::temp_dir());
    let first = engine.build(&parsed(&module(1))).expect("builds");
    engine.install(&first).expect("installs");
    let second = engine.build(&parsed(&module(2))).expect("builds");
    assert_ne!(
        first.digest(),
        second.digest(),
        "a changed body is a new artifact"
    );

    engine.swap(&second).expect("swaps");
    assert_eq!(
        engine.reap(),
        vec![first],
        "no frame was running the first version"
    );
    assert_eq!(engine.reap(), Vec::new(), "it is reported once");
}

#[test]
fn a_module_that_does_not_parse_is_refused_with_its_reason() {
    let mut device = device(true);
    match push(&mut device, "Live", "this is not a module".to_owned()) {
        Frame::Nack(Nack { reason, .. }) => assert_eq!(reason, Reason::VerifyFailed),
        other => panic!("expected a nack, got {other:?}"),
    }
}

#[test]
fn a_layout_change_is_refused_because_the_device_must_restart() {
    let mut device = device(true);
    // The pushed module adds a global, so the running instance's globals and
    // any value crossing into native code no longer line up.
    let widened = module(1).replace(
        "func private @answer() -> i64 engine(runtime) {",
        "global module @added : i64 = 7\n\nfunc private @answer() -> i64 engine(runtime) {",
    );
    match push(&mut device, "Live", widened) {
        Frame::Nack(Nack { reason, .. }) => assert_eq!(reason, Reason::LayoutChanged),
        other => panic!("expected a nack, got {other:?}"),
    }
}

#[test]
fn a_release_device_refuses_every_push() {
    // Section 17.5: hot push is available only in debug bundles.
    let mut device = device(false);
    match push(&mut device, "Live", module(2)) {
        Frame::Nack(Nack {
            reason, message, ..
        }) => {
            assert_eq!(reason, Reason::VerifyFailed);
            assert!(message.contains("debug bundle"), "it says why: {message}");
        }
        other => panic!("expected a nack, got {other:?}"),
    }
}

#[test]
fn the_device_hello_carries_the_layout_of_every_module_it_holds() {
    let mut device = device(true);
    let hello = device.hello("test");
    assert_eq!(hello.version, klf_live::VERSION);
    assert_eq!(hello.bundle, "bundle");
    assert_eq!(hello.layouts.len(), 1, "the one module it holds");

    // A body-only edit leaves the layout alone, which is what lets it through.
    let _ = push(&mut device, "Live", module(5));
    assert!(device.hello("test").agrees_on(&hello, "Live"));
}

#[test]
fn two_sides_that_disagree_about_a_layout_are_told() {
    let mine = Hello {
        version: klf_live::VERSION,
        layouts: vec![("Live".to_owned(), "aaaa".to_owned())],
        ..Hello::default()
    };
    let theirs = Hello {
        version: klf_live::VERSION,
        layouts: vec![("Live".to_owned(), "bbbb".to_owned())],
        ..Hello::default()
    };
    assert!(!mine.agrees_on(&theirs, "Live"));
    assert!(
        mine.agrees_on(&theirs, "Other"),
        "a module neither side holds is new, not a disagreement"
    );
}
/// A module whose `@scaled` C calls through its native entry, multiplying by
/// `factor`, and whose `@main` returns that entry's address. `padding`
/// functions come first, so a version with more of them moves `@scaled`.
fn entered_module(factor: i64, padding: usize) -> String {
    let mut pads = String::new();
    for index in 0..padding {
        let _ = write!(
            pads,
            "func private @pad{index}() -> i64 engine(runtime) {{\nbb0():\n  %zero = const.int i64, 0\n  return %zero\n}}\n\n"
        );
    }
    format!(
        r#"module @Live lang "test" target "aarch64-apple-darwin" stage raw

{pads}func module @scaled(%x: i64) -> i64 engine(runtime) nounwind c(s64 -> s64) {{
bb0(%x: i64):
  %factor = const.int i64, {factor}
  %value = mul %x, %factor
  return %value
}}

func private @main() -> ptr engine(runtime) {{
bb0():
  %entry = func.ref @scaled
  return %entry
}}
"#
    )
}

/// Calls the entry `main` returns the way C would.
#[allow(unsafe_code)]
fn call_entry(device: &mut Device<VmEngine>, argument: i64) -> i64 {
    let RunOutcome::Returned(address) = device
        .engine_mut()
        .run("main", &RunRequest::new())
        .expect("main runs")
    else {
        panic!("main returns the entry's address");
    };
    // SAFETY: `main` returned the address of the VM's native entry for
    // `@scaled`, whose C signature is `s64 -> s64`.
    let entry: extern "C" fn(i64) -> i64 = unsafe {
        std::mem::transmute(usize::try_from(address).expect("an address fits a pointer"))
    };
    entry(argument)
}

#[cfg(any(all(target_arch = "x86_64", not(windows)), target_arch = "aarch64"))]
#[test]
fn c_enters_the_pushed_version_of_a_function_that_moved() {
    let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle(true));
    device
        .install(&parsed(&entered_module(2, 0)))
        .expect("installs with an entry for `@scaled`");
    assert_eq!(call_entry(&mut device, 5), 10);

    let pushed = push(&mut device, "Live", entered_module(3, 2));
    assert!(matches!(pushed, Frame::Ack(_)), "{pushed:?}");
    assert_eq!(call_entry(&mut device, 5), 15);
}

#[test]
fn a_push_changing_what_c_calls_is_refused() {
    let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle(true));
    device
        .install(&parsed(&entered_module(2, 0)))
        .expect("installs");
    // `@scaled` takes a second argument, which C would have to pass.
    let changed = entered_module(2, 0)
        .replace("@scaled(%x: i64)", "@scaled(%x: i64, %y: i64)")
        .replace("bb0(%x: i64)", "bb0(%x: i64, %y: i64)")
        .replace("c(s64 -> s64)", "c(s64, s64 -> s64)");
    let frame = push(&mut device, "Live", changed);
    let Frame::Nack(nack) = frame else {
        panic!("a changed C signature is refused: {frame:?}");
    };
    assert_eq!(nack.reason, Reason::SignatureChanged);
}

#[test]
fn an_empty_debug_device_installs_the_first_push() {
    let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle(true));
    assert_eq!(device.hello("test").layouts, Vec::<(String, String)>::new());

    let installed = push(&mut device, "Live", module(4));
    assert!(
        matches!(&installed, Frame::Ack(Ack { replaced: 2, .. })),
        "{installed:?}"
    );
    assert_eq!(main_of(&mut device), RunOutcome::Returned(4));
    assert_eq!(device.hello("test").layouts.len(), 1);

    let Frame::Nack(nack) = push(&mut device, "Other", module(5)) else {
        panic!("a device holding `Live` does not take another module");
    };
    assert_eq!(nack.reason, Reason::UnknownModule);
}

#[test]
fn an_empty_release_device_installs_nothing() {
    let mut device = Device::new(VmEngine::new(std::env::temp_dir()), bundle(false));
    let Frame::Nack(nack) = push(&mut device, "Live", module(4)) else {
        panic!("a release device takes no push");
    };
    assert_eq!(nack.reason, Reason::VerifyFailed);
    assert_eq!(device.hello("test").layouts, Vec::<(String, String)>::new());
}
