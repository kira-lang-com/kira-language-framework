//! The `lower` answers of earlier builds, kept between runs (section 5.6), and
//! the driver that answers units from them.

use super::*;
use klf_core::FileId;
use klf_diag::progress;
use klf_kir::Module;
use klf_services::{DefinitionPhase, DefinitionUnit, Session};

/// What earlier builds' `lower` queries answered, kept across runs (section
/// 5.6): the module one unit lowered to, or the diagnostics a phase rejected
/// it with. An answer stays valid while every input its phases read is
/// unchanged, so an edit re-runs only the units that read what it changed
/// (section 9.3, requirement 5).
pub trait ModuleCache {
    /// What an earlier build of `unit` kept, when there is an entry for it:
    /// read off the session now, and checked and decoded by the returned
    /// function on any thread.
    fn stored(&self, session: &Session, unit: &DefinitionUnit, program: &[FileId]) -> Option<Stored>;

    /// Keeps the diagnostics the phases of a unit the cache answered kept
    /// when they ran.
    fn answered(&mut self, warnings: String);

    /// Keeps what this build of `unit` answered, with what its phases read
    /// and the diagnostics they kept to answer it, as `trace` spans them.
    fn store(
        &mut self,
        session: &Session,
        unit: &DefinitionUnit,
        program: &[FileId],
        trace: &UnitTrace,
        answer: Result<&Module, (DefinitionPhase, &str)>,
    );
}

/// An entry of a [`ModuleCache`], checked on any thread: what decodes the
/// answer an earlier build kept, when nothing it read has changed since.
pub type Stored = Box<dyn FnOnce() -> Option<Decode> + Send>;

/// Decodes what an entry of a [`ModuleCache`] kept, on any thread.
pub type Decode = Box<dyn FnOnce() -> Option<Kept> + Send>;

/// What an earlier build of a unit answered: the module, or the phase that
/// rejected the unit with its rendered diagnostics, and the diagnostics its
/// phases kept.
pub struct Kept {
    pub answer: Result<Module, (DefinitionPhase, String)>,
    pub warnings: String,
}

/// What one unit's phases added to the session: its observations and its
/// kept diagnostics, as ranges of the session's lists.
#[derive(Clone, Debug)]
pub struct UnitTrace {
    pub observations: std::ops::Range<usize>,
    pub diagnostics: std::ops::Range<usize>,
}

impl UnitTrace {
    fn start(session: &Session) -> Self {
        let observations = session.observations().len();
        let diagnostics = session.runtime_diagnostics().len();
        Self {
            observations: observations..observations,
            diagnostics: diagnostics..diagnostics,
        }
    }

    fn end(mut self, session: &Session) -> Self {
        self.observations.end = session.observations().len();
        self.diagnostics.end = session.runtime_diagnostics().len();
        self
    }
}

/// [`drive`], answering each unit's `lower` query from `cache` when it holds
/// a valid answer, so an edit re-runs only the units that read what it
/// changed. A module from the cache was verified when it was kept; of the
/// merged module only what this build lowered is verified, against the
/// whole of it.
///
/// # Errors
/// Returns [`DriveError`] as [`drive`] does.
pub fn drive_cached<R: PhaseRunner>(
    session: &mut Session,
    inputs: &[FileId],
    make_runner: &mut dyn FnMut() -> Result<R, String>,
    cache: &mut dyn ModuleCache,
) -> Result<Module, DriveError> {
    let units = run_units(session, inputs, make_runner)?;
    let program = program_files(&units);
    let stored: Vec<Option<Stored>> = units
        .iter()
        .map(|unit| cache.stored(session, unit, &program))
        .collect();
    let mut fresh = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut run = |session: &mut Session, unit: &DefinitionUnit, cache: &mut dyn ModuleCache| {
        let trace = UnitTrace::start(session);
        match lower_one(session, unit.clone(), &program, make_runner) {
            Ok(module) => {
                let trace = trace.end(session);
                // What the unit only declares, another unit's module defines
                // and was verified with.
                fresh.extend(
                    module
                        .functions
                        .iter()
                        .filter(|function| !function.foreign)
                        .map(|function| function.name.clone()),
                );
                kept.push((unit.clone(), trace, module.clone()));
                Ok(module)
            }
            Err(DriveError::Phase { phase, message }) => {
                let trace = trace.end(session);
                cache.store(session, unit, &program, &trace, Err((phase, &message)));
                Err(DriveError::Phase { phase, message })
            }
            Err(other) => Err(other),
        }
    };
    // One thread checks every entry and then decodes those that hold, in
    // unit order, while the units without one that holds run their phases,
    // also in unit order. The answers then come in unit order, so the first
    // unit rejected is the one reported.
    let checked: Vec<bool> = stored.iter().map(Option::is_some).collect();
    let lowered = std::thread::scope(|scope| {
        let (told, holds) = std::sync::mpsc::channel();
        let (gave, answers) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let decoders: Vec<Option<Decode>> = stored
                .into_iter()
                .flatten()
                .map(|check| {
                    let decode = check();
                    let _ = told.send(decode.is_some());
                    decode
                })
                .collect();
            for decode in decoders {
                let _ = gave.send(decode.and_then(|decode| decode()));
            }
        });
        // A unit after a rejected one never comes to be answered.
        let mut ran: Vec<Option<Result<Module, DriveError>>> = Vec::new();
        for (unit, checked) in units.iter().zip(&checked) {
            if ran.last().is_some_and(|answer| matches!(answer, Some(Err(_)))) {
                break;
            }
            let held = *checked && holds.recv().unwrap_or(false);
            ran.push((!held).then(|| run(session, unit, cache)));
        }
        let mut lowered = Vec::new();
        for ((unit, checked), ran) in units.iter().zip(&checked).zip(ran) {
            // Every entry's answer comes in turn, whether it held or not.
            let kept = if *checked {
                answers.recv().ok().flatten()
            } else {
                None
            };
            if let Some(answer) = ran {
                lowered.push(answer?);
                continue;
            }
            progress!("query cache");
            match kept {
                Some(Kept {
                    answer: Ok(module),
                    warnings,
                }) => {
                    cache.answered(warnings);
                    lowered.push(module);
                }
                Some(Kept {
                    answer: Err((phase, message)),
                    ..
                }) => return Err(DriveError::Phase { phase, message }),
                // The entry did not decode.
                None => lowered.push(run(session, unit, cache)?),
            }
        }
        Ok(lowered)
    })?;
    progress!("merge");
    let module = merge_units(session, lowered)?;
    if !fresh.is_empty() {
        progress!("verify");
        verify_where(&module, |function| fresh.contains(&function.name))?;
    }
    // A unit's module cannot verify alone, as the functions of the units it
    // depends on are only declared in it, so it is kept once the merged
    // module it joined has.
    if !kept.is_empty() {
        progress!("store");
    }
    for (unit, trace, answer) in &kept {
        cache.store(session, unit, &program, trace, Ok(answer));
    }
    Ok(module)
}
