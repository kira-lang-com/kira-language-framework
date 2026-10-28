//! Where a command's time went, in the units the reader asked for.
//!
//! The measurement half of [`progress`](crate::progress). A phase line is
//! already reported at every boundary worth naming, so timing a build needs no
//! second set of call sites: the recorder listens to the same stream the display
//! draws, closes the previous phase when the next one opens, and adds the
//! durations up.
//!
//! # Why this is not the display
//!
//! The display draws only when a terminal is watching, and it erases itself to
//! stay a fixed height — the two properties that make it useful to watch and
//! useless to measure. A timing report has to survive a pipe, has to keep every
//! phase, and has to be asked for, so it is a separate, independently installed
//! listener on the same feed.
//!
//! # A build is not one thread
//!
//! A phase ends when the *same thread* opens the next one. That is what makes
//! this readable on a build that emits its codegen units in parallel: each
//! thread is credited with its own span rather than with whatever the last
//! thread to speak was doing.
//!
//! What a phase is credited with is **wall-clock**: the span from its first
//! start to its last end, with the gaps between repeats removed. Two phases that
//! ran at once therefore both claim the same milliseconds, and the rows can add
//! up to more than the total — which is the truth about a parallel build, and
//! the reason [`Report::concurrent`] says so rather than leaving a reader to
//! work out why the numbers do not add up.

use std::collections::HashMap;
use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// One phase's share of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// The phase line as it was reported.
    pub name: String,
    /// The wall-clock time this phase was running, on any thread.
    pub elapsed: Duration,
    /// How many times it was entered.
    pub hits: usize,
}

/// A finished recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Wall-clock time from [`start`] to [`take`].
    pub total: Duration,
    /// Every phase, longest first.
    pub phases: Vec<Recorded>,
    /// Time inside the recording that no phase claimed.
    ///
    /// Argument parsing before the first phase, and anything a command does
    /// between the last phase and the end. Kept rather than dropped: a report
    /// whose parts do not add up to its total is a report that hides the part
    /// nobody named.
    pub unattributed: Duration,
    /// Whether two phases ever ran at the same time.
    ///
    /// True for any build that emits its codegen units in parallel. When it is,
    /// the rows overlap each other and sum to more than [`Report::total`].
    pub concurrent: bool,
}

/// The recorder, or `None` when nobody asked for timings.
static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

/// One phase's run on one thread.
struct Span {
    name: String,
    began: Instant,
    ended: Instant,
}

/// Accumulating state between [`start`] and [`take`].
struct Recorder {
    started: Instant,
    /// The phase each thread is currently running, and when it began.
    open: HashMap<ThreadId, (String, Instant)>,
    /// Every phase run that has finished, in the order they finished.
    finished: Vec<Span>,
}

impl Recorder {
    /// Closes `thread`'s open phase at `now`.
    fn close(&mut self, thread: ThreadId, now: Instant) {
        let Some((name, began)) = self.open.remove(&thread) else {
            return;
        };
        self.finished.push(Span {
            name,
            began,
            ended: now,
        });
    }

    /// Closes every thread's open phase at `now`.
    fn close_all(&mut self, now: Instant) {
        for (_thread, (name, began)) in self.open.drain() {
            self.finished.push(Span {
                name,
                began,
                ended: now,
            });
        }
    }
}

/// Begins recording, discarding anything a previous recording left.
///
/// A process runs more than one command, and each is timed on its own.
pub fn start() {
    start_at(Instant::now());
}

/// [`start`], with the instant supplied rather than read from the clock.
///
/// What this module computes is arithmetic over instants, so a test that wants
/// to check the arithmetic supplies them and asserts an exact answer. Sleeping
/// to produce them measures the scheduler instead, which is why the timing
/// tests take this door.
fn start_at(now: Instant) {
    if let Ok(mut slot) = RECORDER.lock() {
        *slot = Some(Recorder {
            started: now,
            open: HashMap::new(),
            finished: Vec::new(),
        });
    }
}

/// Whether a recording is running.
#[must_use]
pub fn recording() -> bool {
    RECORDER.lock().is_ok_and(|slot| slot.is_some())
}

/// Records that `phase` has started, ending whichever phase this thread was
/// running.
///
/// Called from [`progress::report`](crate::progress::report), so no reporter
/// knows this exists.
pub fn mark(phase: &str) {
    mark_at(phase, Instant::now());
}

/// [`mark`], with the instant supplied rather than read from the clock.
fn mark_at(phase: &str, now: Instant) {
    let thread = std::thread::current().id();
    let Ok(mut slot) = RECORDER.lock() else {
        return;
    };
    let Some(recorder) = slot.as_mut() else {
        return;
    };
    recorder.close(thread, now);
    recorder.open.insert(thread, (phase.to_owned(), now));
}

/// The run that finished most recently, taken out of the recording.
///
/// What a log the reader watches as the command runs asks for: the phase that
/// just ended, with the milliseconds it took, so the line can be printed now
/// rather than at the end. [`take`] still sees whatever is left.
#[must_use]
pub fn last_finished() -> Option<Recorded> {
    let mut slot = RECORDER.lock().ok()?;
    let recorder = slot.as_mut()?;
    let span = recorder.finished.pop()?;
    Some(Recorded {
        elapsed: span.ended.saturating_duration_since(span.began),
        hits: 1,
        name: span.name,
    })
}

/// Ends the recording and returns it, or `None` when none was running.
///
/// The phases come back longest first, which is the order the question "why is
/// this slow" is asked in.
#[must_use]
pub fn take() -> Option<Report> {
    take_at(Instant::now())
}

/// [`take`], with the instant supplied rather than read from the clock.
fn take_at(now: Instant) -> Option<Report> {
    let mut recorder = RECORDER.lock().ok()?.take()?;
    recorder.close_all(now);
    let total = now.saturating_duration_since(recorder.started);
    let spans = recorder.finished;

    // First seen first, so a report keeps the command's own order underneath the
    // sort by duration and two phases of equal length do not swap between runs.
    let mut order: Vec<&str> = Vec::new();
    let mut runs: HashMap<&str, Vec<(Instant, Instant)>> = HashMap::new();
    for span in &spans {
        let entry = runs.entry(span.name.as_str()).or_insert_with(|| {
            order.push(span.name.as_str());
            Vec::new()
        });
        entry.push((span.began, span.ended));
    }

    let mut phases: Vec<Recorded> = order
        .into_iter()
        .map(|name| {
            let intervals = &runs[name];
            Recorded {
                name: name.to_owned(),
                elapsed: covered(intervals.clone()),
                hits: intervals.len(),
            }
        })
        .collect();
    phases.sort_by(|left, right| {
        right
            .elapsed
            .cmp(&left.elapsed)
            .then_with(|| left.name.cmp(&right.name))
    });

    let claimed: Duration = phases.iter().map(|phase| phase.elapsed).sum();
    let everything: Vec<(Instant, Instant)> =
        spans.iter().map(|span| (span.began, span.ended)).collect();
    let covered_by_anything = covered(everything);
    Some(Report {
        total,
        phases,
        unattributed: total.saturating_sub(covered_by_anything),
        concurrent: claimed > covered_by_anything,
    })
}

/// How much wall-clock time a set of intervals covers, counting an instant that
/// two of them share only once.
fn covered(mut intervals: Vec<(Instant, Instant)>) -> Duration {
    intervals.sort_by_key(|&(began, _)| began);
    let mut total = Duration::ZERO;
    let mut current: Option<(Instant, Instant)> = None;
    for (began, ended) in intervals {
        current = Some(match current {
            None => (began, ended),
            Some((open_from, open_until)) if began <= open_until => {
                (open_from, open_until.max(ended))
            }
            Some((open_from, open_until)) => {
                total += open_until.saturating_duration_since(open_from);
                (began, ended)
            }
        });
    }
    if let Some((open_from, open_until)) = current {
        total += open_until.saturating_duration_since(open_from);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_is_credited_with_the_time_before_the_next_one() {
        let start = Instant::now();
        start_at(start);
        mark_at("parse", start + Duration::from_millis(10));
        mark_at("lower", start + Duration::from_millis(30));
        let report = take_at(start + Duration::from_millis(40)).expect("a recording");

        let parse = report
            .phases
            .iter()
            .find(|phase| phase.name == "parse")
            .expect("parse");
        // From when it was reported to when the next phase replaced it, not
        // from when the recording started: the ten milliseconds before the first
        // phase belong to no phase and are reported as unattributed.
        assert_eq!(parse.elapsed, Duration::from_millis(20));
        assert_eq!(parse.hits, 1);
        assert_eq!(report.total, Duration::from_millis(40));
        assert!(!report.concurrent, "one thread cannot overlap itself");
    }

    #[test]
    fn the_phases_come_back_longest_first() {
        let start = Instant::now();
        start_at(start);
        mark_at("cheap", start + Duration::from_millis(1));
        mark_at("dear", start + Duration::from_millis(30));
        mark_at("last", start + Duration::from_millis(31));
        let report = take_at(start + Duration::from_millis(40)).expect("a recording");

        // A phase runs until the next one replaces it, so the one reported
        // first and the one running at the end are the long ones.
        assert_eq!(report.phases[0].name, "cheap");
        assert_eq!(report.phases[1].name, "last");
        assert_eq!(report.phases[2].name, "dear");
    }

    #[test]
    fn a_repeated_phase_is_credited_once_per_run_and_counted() {
        let start = Instant::now();
        start_at(start);
        mark_at("codegen", start + Duration::from_millis(0));
        mark_at("link", start + Duration::from_millis(5));
        mark_at("codegen", start + Duration::from_millis(6));
        mark_at("done", start + Duration::from_millis(10));
        let report = take_at(start + Duration::from_millis(10)).expect("a recording");

        let codegen = report
            .phases
            .iter()
            .find(|phase| phase.name == "codegen")
            .expect("codegen");
        assert_eq!(codegen.hits, 2);
        // Two runs of 5ms and 4ms, and the 1ms gap between them counted once.
        assert_eq!(codegen.elapsed, Duration::from_millis(9));
    }

    #[test]
    fn time_no_phase_claimed_is_kept_rather_than_dropped() {
        let start = Instant::now();
        start_at(start);
        mark_at("parse", start + Duration::from_millis(4));
        let report = take_at(start + Duration::from_millis(20)).expect("a recording");

        assert_eq!(report.total, Duration::from_millis(20));
        assert_eq!(report.unattributed, Duration::from_millis(4));
    }

    #[test]
    fn the_run_that_just_ended_is_taken_out_one_at_a_time() {
        let start = Instant::now();
        start_at(start);
        mark_at("parse", start + Duration::from_millis(3));
        mark_at("lower", start + Duration::from_millis(9));

        let parse = last_finished().expect("the run that ended");
        assert_eq!(parse.name, "parse");
        assert_eq!(parse.elapsed, Duration::from_millis(6));
        let report = take_at(start + Duration::from_millis(10)).expect("a recording");
        assert!(
            report.phases.iter().all(|phase| phase.name != "parse"),
            "the taken run is gone from the report"
        );
        assert_eq!(report.total, Duration::from_millis(10));
    }

    #[test]
    fn nothing_is_recorded_unless_it_was_asked_for() {
        let mut slot = RECORDER.lock().expect("the recorder");
        *slot = None;
        drop(slot);
        assert!(!recording());
        mark("a phase nobody times");
        assert!(last_finished().is_none());
        assert!(take().is_none());
    }
}
