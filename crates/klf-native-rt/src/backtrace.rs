//! The backtrace a trap prints (section 16.4).
//!
//! A trap lists the frames that led to it, innermost first: each Kira
//! function by its source name, with its file and line when the program
//! carries the debug information the native backend emits (section 16.3).
//! The frames of the runtime itself, which only print the report, are left
//! out. An image with no symbols at all, as a stripped Linux program is,
//! prints the code addresses on its stack instead (see `stack_code`).
//!
//! `KLF_BACKTRACE=full` lists every frame, the runtime's own included.
//!
//! Capturing walks the stack and resolves it, which allocates, so a guard stops
//! a fault raised inside the capture from trapping again.

use crate::image;
use klf_diag::report::Place;
use std::backtrace::Backtrace;
use std::cell::Cell;

thread_local! {
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
}

/// One frame of a captured backtrace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Frame {
    /// The function, as its source names it.
    pub name: String,
    /// Its file, line and column, when debug information gives them.
    pub place: Option<(String, usize, usize)>,
}

/// Writes the frames of the caller to standard error. A fault raised while
/// they are being taken is not reported twice.
pub(crate) fn report() {
    if CAPTURING.replace(true) {
        return;
    }
    // `KLF_BACKTRACE=full` lists every frame as the platform resolves it,
    // the runtime's own included, as `RUST_BACKTRACE=full` does.
    if std::env::var_os("KLF_BACKTRACE").is_some_and(|value| value == "full") {
        eprintln!("stack backtrace, every frame:\n{}", Backtrace::force_capture());
        CAPTURING.set(false);
        return;
    }
    let frames = captured();
    if frames.is_empty() && !image::present() {
        stack_code::report();
    } else {
        eprint!("{}", listing(&frames));
    }
    CAPTURING.set(false);
}

/// The place of the innermost frame that has one: where a trap the runtime
/// raises by code alone happened.
pub(crate) fn innermost_place() -> Option<Place> {
    if CAPTURING.replace(true) {
        return None;
    }
    let place = captured().into_iter().find_map(|frame| frame.place).map(|(path, line, column)| {
        let source_line = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.lines().nth(line.saturating_sub(1)).map(str::to_owned));
        Place {
            path,
            line,
            column,
            source_line,
            width: 1,
            label: String::new(),
        }
    });
    CAPTURING.set(false);
    place
}

/// The frames of the caller that are the program's own, innermost first.
fn captured() -> Vec<Frame> {
    let mut frames: Vec<Frame> = parse(&Backtrace::force_capture().to_string())
        .into_iter()
        .skip_while(|frame| runtime_frame(&frame.name))
        .collect();
    if let Some(entry) = frames.iter().position(|frame| matches!(frame.name.as_str(), "main" | "_main")) {
        frames.truncate(entry + 1);
    }
    frames
}

/// The frames `std::backtrace::Backtrace` writes, as `N: symbol` lines each
/// followed by an optional `at path:line:column` line.
fn parse(text: &str) -> Vec<Frame> {
    let mut frames: Vec<Frame> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(place) = trimmed.strip_prefix("at ") {
            if let Some(frame) = frames.last_mut().filter(|frame| frame.place.is_none()) {
                frame.place = position(place);
            }
            continue;
        }
        let Some((number, symbol)) = trimmed.split_once(": ") else {
            continue;
        };
        if number.chars().all(|digit| digit.is_ascii_digit()) {
            frames.push(Frame {
                name: crate::names::source_name(symbol.trim()),
                place: None,
            });
        }
    }
    frames
}

fn position(text: &str) -> Option<(String, usize, usize)> {
    let mut parts = text.rsplitn(3, ':');
    let column = parts.next()?.trim().parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    Some((parts.next()?.to_owned(), line, column))
}

/// Whether a frame belongs to the capture and report machinery rather than
/// to the program.
fn runtime_frame(name: &str) -> bool {
    [
        "std::", "<std::", "core::", "<core::", "klf_native_rt::", "klfrt_trap", "_klfrt_trap",
        "_sigtramp", "__sigtramp", "__restore_rt", "_os_", "_platform_",
    ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// How many of the innermost and outermost frames a long backtrace shows.
const SHOWN_INNER: usize = 40;
const SHOWN_OUTER: usize = 10;

/// The frames as a trap report lists them.
fn listing(frames: &[Frame]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("stack backtrace, innermost first:\n");
    for (index, frame) in frames.iter().enumerate() {
        // A runaway recursion repeats the same frames: the innermost and
        // the outermost say where it started and where it stopped.
        if frames.len() > SHOWN_INNER + SHOWN_OUTER && index == SHOWN_INNER {
            let hidden = frames.len() - SHOWN_INNER - SHOWN_OUTER;
            let _ = writeln!(out, "      ... {hidden} more frames ...");
        }
        if frames.len() > SHOWN_INNER + SHOWN_OUTER
            && index >= SHOWN_INNER
            && index < frames.len() - SHOWN_OUTER
        {
            continue;
        }
        let _ = writeln!(out, "{index:>4}: {}", frame.name);
        if let Some((path, line, column)) = &frame.place {
            let _ = writeln!(out, "             at {path}:{line}:{column}");
        }
    }
    if frames.iter().all(|frame| frame.place.is_none()) {
        out.push_str(
            "      (no file and line: the program was built without debug information)\n",
        );
    }
    out
}

/// Up to `limit` code addresses on the caller's stack, innermost first, to be
/// printed later with [`print_words`].
pub(crate) fn capture(limit: usize) -> Vec<u64> {
    stack_code::capture(limit)
}

/// Writes addresses taken by [`capture`] as their images and offsets.
pub(crate) fn print_words(words: &[u64]) {
    stack_code::print_words(words);
}

/// A stripped program's trap, located: every word on the trapping thread's
/// stack that points into an executable segment of a loaded image, printed
/// as that image's path and the address within it. The return addresses of
/// the frames that led to the trap are among them, innermost first, and a
/// separate symbol file (a dSYM's DWARF) names each with
/// `llvm-symbolizer --obj=<DWARF> <address>`. Words that only happen to look
/// like code addresses come out too; the frames are the ones that symbolize
/// to a call.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod stack_code {
    use std::ffi::{CStr, c_char, c_int, c_void};

    /// How many addresses are printed at most.
    const LIMIT: usize = 96;
    /// How far up the stack is read at most, in bytes.
    const DEPTH: usize = 256 * 1024;
    const LOAD: u32 = 1;
    const EXECUTE: u32 = 1;

    #[repr(C)]
    struct ProgramHeader {
        kind: u32,
        flags: u32,
        offset: u64,
        address: u64,
        physical: u64,
        file_size: u64,
        memory_size: u64,
        align: u64,
    }

    #[repr(C)]
    struct LoadedImage {
        base: u64,
        name: *const c_char,
        headers: *const ProgramHeader,
        count: u16,
    }

    unsafe extern "C" {
        fn dl_iterate_phdr(
            callback: unsafe extern "C" fn(*mut LoadedImage, usize, *mut c_void) -> c_int,
            data: *mut c_void,
        ) -> c_int;
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attributes: *mut [u64; 16]) -> c_int;
        fn pthread_attr_getstack(
            attributes: *const [u64; 16],
            address: *mut *mut c_void,
            size: *mut usize,
        ) -> c_int;
        fn pthread_attr_destroy(attributes: *mut [u64; 16]) -> c_int;
    }

    /// An executable segment: where it is, the base its image was loaded at,
    /// and the image's path.
    struct Segment {
        start: u64,
        end: u64,
        base: u64,
        name: String,
    }

    unsafe extern "C" fn collect(image: *mut LoadedImage, _: usize, data: *mut c_void) -> c_int {
        // SAFETY: the loader hands a valid image record, and `data` is the
        // vector `segments` passed below.
        let (image, segments) = unsafe { (&*image, &mut *data.cast::<Vec<Segment>>()) };
        let name = if image.name.is_null() {
            String::new()
        } else {
            // SAFETY: a non-null name is a C string the loader owns.
            unsafe { CStr::from_ptr(image.name) }
                .to_string_lossy()
                .into_owned()
        };
        let name = if name.is_empty() {
            std::env::current_exe()
                .map_or_else(|_| "program".into(), |path| path.display().to_string())
        } else {
            name
        };
        for index in 0..usize::from(image.count) {
            // SAFETY: `count` headers follow `headers`.
            let header = unsafe { &*image.headers.add(index) };
            if header.kind == LOAD && header.flags & EXECUTE != 0 {
                let start = image.base.wrapping_add(header.address);
                segments.push(Segment {
                    start,
                    end: start.wrapping_add(header.memory_size),
                    base: image.base,
                    name: name.clone(),
                });
            }
        }
        0
    }

    /// The lowest address above the caller's frame and the top of this
    /// thread's stack, or nothing where the stack's extent is not known.
    fn stack() -> Option<(usize, usize)> {
        let marker = 0_u64;
        let here = std::ptr::addr_of!(marker) as usize;
        let mut attributes = [0_u64; 16];
        let mut address = std::ptr::null_mut();
        let mut size = 0_usize;
        // SAFETY: the attribute buffer is larger than any libc's
        // `pthread_attr_t`, and is destroyed after it is read.
        unsafe {
            if pthread_getattr_np(pthread_self(), &raw mut attributes) != 0 {
                return None;
            }
            let read =
                pthread_attr_getstack(&raw const attributes, &raw mut address, &raw mut size);
            pthread_attr_destroy(&raw mut attributes);
            if read != 0 {
                return None;
            }
        }
        let top = (address as usize).checked_add(size)?;
        if here >= top {
            return None;
        }
        Some((here, top.min(here.saturating_add(DEPTH))))
    }

    fn loaded() -> Vec<Segment> {
        let mut segments: Vec<Segment> = Vec::new();
        // SAFETY: `collect` reads only what the loader hands it.
        unsafe { dl_iterate_phdr(collect, (&raw mut segments).cast()) };
        segments
    }

    fn scan(segments: &[Segment], limit: usize) -> Vec<u64> {
        let Some((low, high)) = stack() else {
            return Vec::new();
        };
        let mut words = Vec::new();
        let mut at = low.next_multiple_of(8);
        while at + 8 <= high && words.len() < limit {
            // SAFETY: `at` lies inside this thread's stack, which is mapped.
            let word = unsafe { std::ptr::read_volatile(at as *const u64) };
            if segments.iter().any(|s| word >= s.start && word < s.end) {
                words.push(word);
            }
            at += 8;
        }
        words
    }

    fn print(segments: &[Segment], words: &[u64]) {
        for &word in words {
            if let Some(segment) = segments.iter().find(|s| word >= s.start && word < s.end) {
                eprintln!("  {}+{:#x}", segment.name, word - segment.base);
            }
        }
    }

    pub(super) fn report() {
        let segments = loaded();
        eprintln!("code addresses on the stack, innermost first:");
        print(&segments, &scan(&segments, LIMIT));
    }

    pub(super) fn capture(limit: usize) -> Vec<u64> {
        static SEGMENTS: std::sync::OnceLock<Vec<Segment>> = std::sync::OnceLock::new();
        scan(SEGMENTS.get_or_init(loaded), limit)
    }

    pub(super) fn print_words(words: &[u64]) {
        print(&loaded(), words);
    }
}

/// Elsewhere a stripped program's trap prints no frames.
#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
mod stack_code {
    pub(super) fn report() {}

    pub(super) fn capture(_: usize) -> Vec<u64> {
        Vec::new()
    }

    pub(super) fn print_words(_: &[u64]) {}
}

#[cfg(test)]
mod tests {
    use super::{Frame, parse, report};

    #[test]
    fn a_report_can_be_written_twice() {
        report();
        report();
    }

    #[test]
    fn frames_read_as_their_source_names_and_places() {
        let text = "   0: _kir.pick#TrapGallery\n             at ./app/main.kira:8:12\n   1: main\n";
        assert_eq!(
            parse(text),
            vec![
                Frame {
                    name: "TrapGallery.pick".to_owned(),
                    place: Some(("./app/main.kira".to_owned(), 8, 12)),
                },
                Frame {
                    name: "main".to_owned(),
                    place: None,
                },
            ]
        );
    }
}
