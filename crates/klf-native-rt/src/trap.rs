//! Traps (sections 16.4 and 18.8).
//!
//! A trap ends the program with a `KLF-RT` code. A process reports it on
//! standard error as a Rust compiler error reads: what went wrong, where, the
//! values involved, why the rule holds and how to fix the source, then the
//! frames that led there. A language check carries that report from compile
//! time (`klfrt_trap_message`); a trap the runtime or a backend raises by code
//! alone is described here, at the innermost Kira frame that has a place.
//!
//! A test run and the VM's outcome protocol still read `TRAP <code>` on
//! standard output.

use crate::{backtrace, ending, interrupt, is_process};
use klf_diag::report::{Report, fill_operands, report_code};
use std::cell::Cell;
use std::sync::atomic::Ordering;

thread_local! {
    /// The values the next trap's report names as `{0}` and `{1}`.
    static OPERANDS: Cell<[i64; 2]> = const { Cell::new([0; 2]) };
    static OPERAND_COUNT: Cell<usize> = const { Cell::new(0) };
    /// The text the next trap's report names as `{text}`.
    static TEXT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Kernel service `klf.trap.text`: `length` bytes of UTF-8 the report of
/// the trap that follows names, such as the type an `Any` held.
///
/// # Safety
///
/// `data` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_trap_text(data: *const u8, length: i64) {
    let Ok(length) = usize::try_from(length) else {
        return;
    };
    let bytes = if length == 0 || data.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller passes a buffer of `length` bytes.
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    TEXT.with(|text| *text.borrow_mut() = Some(String::from_utf8_lossy(bytes).into_owned()));
}

/// The code of a trap a language check raised without a code of its own.
const CHECK_FAILED: i32 = 7;

/// Kernel service `klf.trap.operands`: the values a check compared, which
/// the report of the trap that follows names.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_trap_operands(first: i64, second: i64) {
    OPERANDS.with(|operands| operands.set([first, second]));
    OPERAND_COUNT.with(|count| count.set(2));
}

fn operands() -> Vec<i64> {
    let count = OPERAND_COUNT.with(|count| count.replace(0));
    OPERANDS.with(Cell::get)[..count].to_vec()
}

/// Ends the program with trap `code`, matching `Outcome::Trap`.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_trap(code: i32) -> ! {
    trap_with(code, Vec::new())
}

/// [`klfrt_trap`], with notes naming the values involved.
pub(crate) fn trap_with(code: i32, details: Vec<String>) -> ! {
    interrupt::raise_if_serving(interrupt::Interrupt::Trapped(code));
    ending::run_exit_function(ending::TRAPPED, i64::from(code));
    if is_process() {
        let mut report = described(code);
        report.notes.splice(0..0, details);
        report.place = backtrace::innermost_place();
        print(&report.render());
    }
    println!("TRAP KLF-RT-{code:04}");
    std::process::exit(0)
}

/// A trap a language check raised: `message`, `length` bytes of UTF-8, is
/// the report the compiler rendered for it, or for code built without one,
/// the source that failed and where it stands.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_trap_message(message: *const u8, length: usize) -> ! {
    // SAFETY: the code a backend emits passes a constant string it placed in
    // the image, with its byte length.
    let bytes = unsafe { std::slice::from_raw_parts(message, length) };
    let text = String::from_utf8_lossy(bytes);
    let rendered = match report_code(&text) {
        Some(_) => {
            let named = TEXT.with(|text| text.borrow_mut().take());
            klf_diag::report::fill_text(&fill_operands(&text, &operands()), named.as_deref())
        }
        None => checked_source(&text),
    };
    let code = report_code(&rendered)
        .and_then(|code| code.strip_prefix("KLF-RT-"))
        .and_then(|number| number.parse().ok())
        .unwrap_or(CHECK_FAILED);
    interrupt::raise_if_serving(interrupt::Interrupt::CheckFailed(rendered.clone()));
    ending::run_exit_function(ending::TRAPPED, i64::from(code));
    if is_process() {
        print(&rendered);
    }
    println!("TRAP {}", klf_diag::report::outcome_line(&rendered));
    std::process::exit(0)
}

/// The report of a check that carries only the source it checked, as
/// `path:line:column: source`.
fn checked_source(text: &str) -> String {
    let mut report = Report {
        code: format!("KLF-RT-{CHECK_FAILED:04}"),
        headline: format!("a check failed: `{text}`"),
        ..Report::default()
    };
    if let Some((place, source)) = text.split_once(": ") {
        let mut parts = place.rsplitn(3, ':');
        let column = parts.next().and_then(|part| part.parse().ok());
        let line = parts.next().and_then(|part| part.parse().ok());
        if let (Some(column), Some(line), Some(path)) = (column, line, parts.next()) {
            report.headline = format!("a check failed: `{source}`");
            report.place = Some(klf_diag::report::Place {
                path: path.to_owned(),
                line,
                column,
                ..klf_diag::report::Place::default()
            });
        }
    }
    report
        .notes
        .push("the compiler checks this source at run time, and the check did not hold".to_owned());
    report.helps.push(
        "the frames below lead to the check; rebuild with the current compiler for a report that \
         says which check it is"
            .to_owned(),
    );
    report.render()
}

/// Writes a report and the frames that led to it, then ends the process as
/// the program asked: exit status 1, or an abort.
fn print(rendered: &str) -> ! {
    eprint!("{rendered}");
    backtrace::report();
    if crate::TRAP_EXITS.load(Ordering::Relaxed) {
        std::process::exit(1)
    }
    std::process::abort()
}

/// The report of trap `code` where it is raised: what it means, why it
/// happens, how to fix it and the innermost place the frames name.
#[must_use]
pub fn trap_report(code: i32) -> String {
    let mut report = described(code);
    report.place = backtrace::innermost_place();
    report.render()
}

/// What trap `code` means, why it happens and how to fix it (section 18.8).
fn described(code: i32) -> Report {
    klf_diag::report::trap_description(&format!("KLF-RT-{code:04}"))
}
