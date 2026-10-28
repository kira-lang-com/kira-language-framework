//! The millisecond log `--timings` prints instead of the surface.
//!
//! The surface animates and then erases itself, which is exactly wrong for a
//! record: it survives the run, it reads in the order the phases ran, and it
//! needs no terminal to be worth having. So under this flag the surface is never
//! installed — one line per phase goes to stdout as that phase ends, and the
//! total closes the list.

use std::sync::Arc;
use std::time::Duration;

use crate::progress::ProgressSink;
use crate::timeline;

/// Starts the recording and installs the log that prints from it.
///
/// What the command was doing is titled into the closing line, so the same
/// title names the whole report.
#[must_use]
pub fn install(title: &str) -> Timings {
    timeline::start();
    let log = Arc::new(Log {
        title: title.to_owned(),
    });
    crate::progress::install(log.clone());
    Timings { log }
}

/// A timings run in progress: the log every phase reports into, and the guard
/// that prints what is left of it.
pub struct Timings {
    log: Arc<Log>,
}

/// The listening half of a timings run: one line per phase, as it ends.
struct Log {
    title: String,
}

impl Timings {
    /// Closes the report, however the command returned.
    pub fn end(self) {
        self.log.finish();
    }
}

impl Log {
    /// Ends the recording and prints the phases that had not finished yet, plus
    /// the total.
    fn finish(&self) {
        crate::progress::uninstall();
        let Some(report) = timeline::take() else {
            return;
        };
        // Reverse order: the recorder hands phases back longest first, because
        // that is the order the question "why is this slow" is asked in, and this
        // list reads in the order the command ran.
        for phase in report.phases.iter().rev() {
            line(&phase.name, phase.elapsed, phase.hits);
        }
        // Time the recording kept that no phase claimed is kept in the list
        // rather than dropped: a reader adding the rows up has to be able to see
        // that they add up to the total.
        if !report.unattributed.is_zero() {
            line("(unattributed)", report.unattributed, 0);
        }
        // The total always goes out, even at zero.
        crate::out!("{}: {}ms total", self.title, milliseconds(report.total));
        if report.concurrent {
            crate::out!("  phases overlapped: the rows are wall-clock and sum past the total");
        }
    }
}

impl ProgressSink for Log {
    fn phase(&self, _phase: &str) {
        // The phase this one replaced has just ended, and the recorder is what
        // knows how long it ran. Nothing to print on the first phase of a
        // command: nothing has finished yet.
        if let Some(done) = timeline::last_finished() {
            line(&done.name, done.elapsed, done.hits);
        }
    }

    fn suspend(&self) {}
}

/// One row of the log: how long the phase ran, then what it was.
fn line(name: &str, elapsed: Duration, hits: usize) {
    let repeats = if hits > 1 {
        format!(" ×{hits}")
    } else {
        String::new()
    };
    crate::out!("  {:>7}ms  {name}{repeats}", milliseconds(elapsed));
}

/// A duration in milliseconds, to one decimal.
///
/// One decimal because the phases being compared are usually a few dozen
/// milliseconds long, and a whole number would round half of them to `0ms` and
/// answer no question.
fn milliseconds(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_reads_in_milliseconds() {
        assert_eq!(milliseconds(Duration::from_millis(1500)), 1500.0);
        assert_eq!(milliseconds(Duration::ZERO), 0.0);
    }

    #[test]
    fn a_phase_short_enough_to_round_away_still_reads() {
        assert!((milliseconds(Duration::from_micros(400)) - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_fractional_millisecond_is_not_printed_as_a_runt_of_digits() {
        // What `as_secs_f64` gives for 60 microseconds, printed unrounded: the
        // rows are read by eye and by `awk`, not by `f64` equality.
        let unrounded = Duration::from_micros(60).as_secs_f64() * 1000.0;
        assert_ne!(unrounded, milliseconds(Duration::from_micros(60)));
        assert_eq!(milliseconds(Duration::from_micros(60)), 0.1);
    }

    #[test]
    fn a_recording_is_taken_once_so_a_command_prints_one_total() {
        // `take` empties the recording, so a second `finish` has nothing to say
        // and a command that reports in two halves does not print two totals.
        timeline::start();
        assert!(timeline::take().is_some());
        assert!(timeline::take().is_none());
    }
}
