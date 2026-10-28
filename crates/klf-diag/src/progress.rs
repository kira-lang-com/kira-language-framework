//! The channel a build reports its phases on.
//!
//! A command that prints nothing while it works is indistinguishable from one
//! that has hung, and the only person who can tell them apart is the one
//! holding a stopwatch. This is how each phase says it started.
//!
//! # Why a process-level sink
//!
//! Progress is not analysis. Nothing reads it back, nothing branches on it, and
//! a build that reports nothing produces byte-identical artifacts to one that
//! reports everything — so threading a reporter through every signature would
//! buy correctness that is already guaranteed and cost a parameter on every
//! function between the CLI and the linker. The sink is installed once, by the
//! binary that owns the terminal, and every layer below reports into it without
//! knowing whether anyone is listening.
//!
//! With no sink installed [`report`] does nothing, which is what a library
//! consumer and a test both want.

use std::sync::{Arc, RwLock};

/// Something that displays a build's phases.
pub trait ProgressSink: Send + Sync {
    /// Reports that `phase` has started.
    fn phase(&self, phase: &str);

    /// Takes the display down so something else can write.
    ///
    /// A display that redraws in place and a diagnostic printed underneath it
    /// interleave into nonsense — half a status block, a note, then a status
    /// block that scrolled. Anything writing its own output says so first.
    fn suspend(&self);

    /// Puts the display back once whatever wrote its own output is done.
    ///
    /// Optional: a display that redraws on a timer of its own comes back on the
    /// next tick without being told, and one that only redraws on the next phase
    /// has nothing to restore.
    fn resume(&self) {}
}

/// Reports a phase, built from a format string.
///
/// A phase line usually names a file or a count, and building that string for a
/// build nobody is watching is work with no reader, so the format arguments are
/// only evaluated when something is listening.
#[macro_export]
macro_rules! progress {
    ($($argument:tt)*) => {
        if $crate::progress::watched() {
            $crate::progress::report(&format!($($argument)*));
        }
    };
}

/// The installed sink, or `None` when nobody is listening.
///
/// An `RwLock` rather than a `OnceLock`: a process runs more than one build,
/// and each installs its own display and takes it down again.
static SINK: RwLock<Option<Arc<dyn ProgressSink>>> = RwLock::new(None);

/// Installs `sink` as the destination for every later [`report`].
pub fn install(sink: Arc<dyn ProgressSink>) {
    if let Ok(mut slot) = SINK.write() {
        *slot = Some(sink);
    }
}

/// Removes the installed sink, so later reports go nowhere.
pub fn uninstall() {
    if let Ok(mut slot) = SINK.write() {
        *slot = None;
    }
}

/// Reports that `phase` has started.
///
/// Two listeners, installed independently: the display, which draws it, and the
/// [`timeline`](crate::timeline), which times it. A build watched by neither
/// pays one uncontended read lock and a `None`. A poisoned lock is treated as no
/// listener rather than a panic — losing a progress line is never worth failing
/// a build over.
pub fn report(phase: &str) {
    crate::timeline::mark(phase);
    let Ok(slot) = SINK.read() else {
        return;
    };
    if let Some(sink) = slot.as_ref() {
        sink.phase(phase);
    }
}

/// Takes the installed display down for the life of the returned guard.
///
/// Nothing to restore: the surface redraws itself on the next phase, and a
/// build that reports no further phase had nothing left to show anyway.
pub fn suspended() -> Suspended {
    if let Ok(slot) = SINK.read()
        && let Some(sink) = slot.as_ref()
    {
        sink.suspend();
    }
    Suspended
}

/// The guard [`suspended`] returns.
#[derive(Debug)]
#[must_use = "the display stays down only while the guard is alive"]
pub struct Suspended;

/// Puts the installed display back, after something wrote its own output.
///
/// What a caller that prints under [`suspended`] calls when it is done: without
/// it a display that only redraws on a phase would stay down for the rest of the
/// command, and the reader would think the build had finished.
pub fn resumed() {
    if let Ok(slot) = SINK.read()
        && let Some(sink) = slot.as_ref()
    {
        sink.resume();
    }
}

/// Whether a sink is installed.
#[must_use]
pub fn listening() -> bool {
    SINK.read().is_ok_and(|slot| slot.is_some())
}

/// Whether anything at all consumes phases — a display, a recording, or both.
///
/// What [`progress!`] gates its formatting on. Asking about the display alone
/// would leave `--timings` measuring only the phases whose lines are constants,
/// which is most of the cheap ones and none of the ones that name a file.
#[must_use]
pub fn watched() -> bool {
    listening() || crate::timeline::recording()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serializes the tests that install a sink.
    ///
    /// The sink is process-global, so two of these running at once trade
    /// installs and reports: one test's `uninstall` silences another's, and one
    /// test's phase lands in another's recorder.
    fn exclusive() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A sink that remembers what it was told.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl ProgressSink for Recorder {
        fn phase(&self, phase: &str) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push(phase.to_owned());
            }
        }

        fn suspend(&self) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push("<suspended>".to_owned());
            }
        }

        fn resume(&self) {
            if let Ok(mut seen) = self.0.lock() {
                seen.push("<resumed>".to_owned());
            }
        }
    }

    #[test]
    fn reporting_with_no_sink_installed_does_nothing() {
        let _exclusive = exclusive();
        uninstall();
        assert!(!listening());
        report("a phase nobody hears");
    }

    #[test]
    fn an_installed_sink_receives_every_phase() {
        let _exclusive = exclusive();
        let recorder = Arc::new(Recorder::default());
        install(recorder.clone());
        report("parsing");
        report("linking");
        uninstall();
        let seen = recorder.0.lock().expect("the recorder");
        assert!(seen.contains(&"parsing".to_owned()), "{seen:?}");
        assert!(seen.contains(&"linking".to_owned()), "{seen:?}");
    }

    #[test]
    fn suspending_reaches_the_installed_sink() {
        let _exclusive = exclusive();
        let recorder = Arc::new(Recorder::default());
        install(recorder.clone());
        let _guard = suspended();
        uninstall();
        let seen = recorder.0.lock().expect("the recorder");
        assert!(seen.contains(&"<suspended>".to_owned()), "{seen:?}");
    }

    #[test]
    fn resuming_reaches_the_installed_sink() {
        let _exclusive = exclusive();
        let recorder = Arc::new(Recorder::default());
        install(recorder.clone());
        resumed();
        uninstall();
        let seen = recorder.0.lock().expect("the recorder");
        assert!(seen.contains(&"<resumed>".to_owned()), "{seen:?}");
    }

    #[test]
    fn suspending_with_no_sink_installed_does_nothing() {
        let _exclusive = exclusive();
        uninstall();
        let _guard = suspended();
        resumed();
    }

    #[test]
    fn nothing_watches_when_nothing_is_installed() {
        let _exclusive = exclusive();
        uninstall();
        assert!(!watched());
    }
}
