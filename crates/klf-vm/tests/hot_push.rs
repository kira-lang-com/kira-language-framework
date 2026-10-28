//! Section 17.5's two-version semantics: a pushed module's code is what new
//! calls enter, the version frames already running keep running to the end, and
//! a third push into a slot whose frames have not returned is refused as busy.

use klf_kir::parse_module;
use klf_vm::{Capabilities, Code, Machine, Outcome, lower_module};
use std::fmt::Write as _;

fn program(text: &str) -> klf_vm::Program {
    lower_module(&parse_module(text).expect("module parses")).expect("module lowers")
}

/// `@answer` returning 1 before the push and 2 after it, with `@main` calling
/// it, so one edit to one body is a whole new version of the module.
fn version_source(answer: i64) -> String {
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

fn call(machine: &mut Machine<'_>, name: &str) -> Outcome {
    let entry = machine.function_index(name).expect("entry exists");
    machine.call(entry, &[], &Capabilities::new())
}

#[test]
fn a_pushed_version_is_what_the_next_call_enters() {
    let first = program(&version_source(1));
    let mut machine = Machine::new(&first);
    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(1)));

    let pushed = Code::new(program(&version_source(2)));
    machine
        .adopt(std::sync::Arc::new(pushed))
        .expect("first push takes the free slot");

    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(2)));
    assert_eq!(machine.version(), 1);
}

#[test]
fn globals_keep_their_values_across_a_push() {
    // The pushed module declares the same global with the same shape, so the
    // address a running program already holds still names it.
    let source = |value: i64| {
        format!(
            r#"module @Live lang "test" target "aarch64-apple-darwin" stage raw

global module @counter : i64 = {value}

func private @main() -> i64 engine(runtime) {{
bb0():
  %address = global.addr @counter
  %value = load [trivial] %address
  return %value
}}
"#
        )
    };
    let first = program(&source(41));
    let mut machine = Machine::new(&first);
    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(41)));

    let pushed = Code::new(program(&source(42)));
    machine
        .adopt(std::sync::Arc::new(pushed))
        .expect("same layout, so the push installs");

    // The global was written once at machine start and never re-initialized, so
    // the value the running instance already held is the one still read.
    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(41)));
}

#[test]
fn a_third_push_is_refused_while_the_retired_version_still_runs() {
    let first = program(&version_source(1));
    let mut machine = Machine::new(&first);
    let second = Code::new(program(&version_source(2)));
    let third = Code::new(program(&version_source(3)));

    // Nothing is running, so both pushes land and the older slot is reaped.
    machine
        .adopt(std::sync::Arc::new(second))
        .expect("slot 1 is free");
    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(2)));
    machine
        .adopt(std::sync::Arc::new(third))
        .expect("slot 0 is free again");
    assert_eq!(call(&mut machine, "main"), Outcome::Return(Some(3)));
    assert_eq!(machine.version(), 0);
}

#[test]
fn a_version_is_reaped_once_no_frame_entered_it() {
    let first = program(&version_source(1));
    let mut machine = Machine::new(&first);
    machine
        .adopt(std::sync::Arc::new(Code::new(program(&version_source(2)))))
        .expect("first push");
    assert!(
        !machine.version_busy(0),
        "a finished call leaves nothing running"
    );
    machine.reap();
    assert_eq!(machine.version(), 1);
}

/// A module calling the natives in `order`.
/// The VM numbers imports by first call, so the order is the import table's.
fn foreign_source(order: &[&str]) -> String {
    let mut declarations = String::new();
    let mut calls = String::new();
    for name in order {
        let _ = writeln!(
            declarations,
            "func private foreign @{name}() -> i64 engine(native)"
        );
        let _ = writeln!(calls, "  %{name} = call.foreign @{name}()");
    }
    format!(
        r#"module @Live lang "c" target "aarch64-apple-darwin" stage canonical

{declarations}
func module @main() -> i64 engine(native) {{
bb0():
{calls}  %zero = const.int i64, 0
  return %zero
}}
"#
    )
}

/// The symbols a program's foreign call sites reach, in call order.
fn called(program: &klf_vm::Program) -> Vec<String> {
    program
        .functions
        .iter()
        .flat_map(|function| &function.call_sites)
        .filter_map(|site| site.foreign)
        .map(|index| program.imports[index as usize].symbol.clone())
        .collect()
}

#[test]
fn a_pushed_version_calls_its_natives_through_the_running_import_table() {
    let running = program(&foreign_source(&["first", "second"]));
    let pushed = program(&foreign_source(&["second", "first"]));
    assert_ne!(
        pushed.imports, running.imports,
        "the push imports its natives in another order"
    );
    let expected = called(&pushed);

    let rebased =
        klf_vm::rebase_imports(pushed, &running).expect("every native is already imported");
    assert_eq!(rebased.imports, running.imports);
    assert_eq!(called(&rebased), ["second", "first"]);
    assert_eq!(called(&rebased), expected);
}

#[test]
fn a_pushed_version_needing_a_new_native_is_refused() {
    let running = program(&foreign_source(&["first", "second"]));
    let pushed = program(&foreign_source(&["first", "second", "third"]));
    let refusal = klf_vm::rebase_imports(pushed, &running).expect_err("nothing resolved `third`");
    assert!(refusal.contains("third"), "{refusal}");
}
