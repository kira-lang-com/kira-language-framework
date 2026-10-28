//! A cancelled run stops at its next call or loop back edge and unwinds its
//! frames (`KLF-EDITOR.md` sections 8.6 and 24.1).

use klf_kir::parse_module;
use klf_vm::{CANCELLED, Capabilities, Machine, Outcome, lower_module};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SPIN: &str = r#"module @Spin lang "test" target "aarch64-apple-darwin" stage raw

func private @spin() -> i64 engine(runtime) {
bb0():
  %one = const.int i64, 1
  br bb1(%one)
bb1(%i: i64):
  %next = add %i, %one
  br bb1(%next)
}

func private @deep(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %done = icmp eq %n, %zero
  cond.br %done, bb1(), bb2()
bb1():
  return %zero
bb2():
  %one = const.int i64, 1
  %less = sub %n, %one
  %r = call @deep(%less)
  return %r
}

func private @recurse() -> i64 engine(runtime) {
bb0():
  %depth = const.int i64, 4000
  br bb1()
bb1():
  %r = call @deep(%depth)
  br bb1()
}
"#;

/// Runs `entry` of [`SPIN`], cancelling it after `after`, and answers its
/// outcome, how long it ran past the cancellation, and whether its heap is
/// balanced.
fn cancel(entry: &str, after: Duration) -> (Outcome, Duration, bool) {
    let module = parse_module(SPIN).expect("module parses");
    let program = lower_module(&module).expect("module lowers");
    let index = program.function_index(entry).expect("entry exists");
    let mut machine = Machine::new(&program);
    let flag = Arc::new(AtomicBool::new(false));
    machine.cancel_on(Arc::clone(&flag));
    let cancelled_at = std::thread::scope(|scope| {
        let cancelling = scope.spawn(|| {
            std::thread::sleep(after);
            flag.store(true, Ordering::Release);
            Instant::now()
        });
        let outcome = machine.call(index, &[], &Capabilities::new());
        let at = cancelling.join().expect("the cancelling thread ends");
        (outcome, at)
    });
    let (outcome, at) = cancelled_at;
    (outcome, at.elapsed(), machine.balanced())
}

#[test]
fn a_long_loop_stops_at_a_back_edge() {
    let (outcome, released, balanced) = cancel("spin", Duration::from_millis(20));
    assert_eq!(outcome, Outcome::Trap(CANCELLED.to_owned()));
    assert!(
        released < Duration::from_millis(20),
        "released after {released:?}"
    );
    assert!(balanced, "the heap is balanced after the cancellation");
}

#[test]
fn deep_recursion_stops_at_a_call() {
    let (outcome, released, balanced) = cancel("recurse", Duration::from_millis(20));
    assert_eq!(outcome, Outcome::Trap(CANCELLED.to_owned()));
    assert!(
        released < Duration::from_millis(20),
        "released after {released:?}"
    );
    assert!(balanced, "the heap is balanced after the cancellation");
}

#[test]
fn an_uncancelled_machine_runs_as_before() {
    let module = parse_module(SPIN).expect("module parses");
    let program = lower_module(&module).expect("module lowers");
    let index = program.function_index("deep").expect("entry exists");
    let mut machine = Machine::new(&program);
    machine.cancel_on(Arc::new(AtomicBool::new(false)));
    let outcome = machine.call(index, &[100], &Capabilities::new());
    assert_eq!(outcome, Outcome::Return(Some(0)));
}
