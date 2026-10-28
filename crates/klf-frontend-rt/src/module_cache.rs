//! The driver's side of the cross-build query cache (section 5.6): what the
//! `lower` query answered for each unit, kept under `$KLF_CACHE/queries`.
//!
//! An entry's file name is the SHA-256 of its key: the KLF release, the
//! definition hash and this frontend binary, the target, the profile (what to
//! emit, the flags set and the file-mode selections), the query, and the name
//! and SHA-256 of each of the unit's files.
//! The entry holds what the phases then read from the file system, with the
//! SHA-256 of each file, the diagnostics they kept, and the answer: the module
//! as `.kirb`, or the phase that rejected the unit with its diagnostics. It
//! answers a later build only when every file still hashes the same, every
//! path still exists or not as it did, and every directory lists the same
//! files: red-green validation of the `lower` query (section 8.9).
//!
//! A `.ridl` contract is kept not by its whole text but by the declarations
//! the unit reaches of it, from the names its sources spell
//! ([`crate::contract_reach`]): a declaration added that the unit cannot name
//! leaves its entry answering.
//!
//! Entries live in one directory per slot, the unit's file names under the
//! target and profile, and a new entry replaces the others of its slot: an
//! edited package or a rebuilt frontend makes the older entries unreachable.

use klf_defsvc::{
    Decode, FrontendMetadata, Kept, ModuleCache, Stored, UnitTrace, render_diagnostics,
    render_kept_diagnostics,
};
use klf_core::FileId;
use klf_kir::Module;
use klf_ridl::sha256::{hex, sha256};
use klf_services::{DefinitionPhase, DefinitionUnit, Observation, Session};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

/// One thing the phases observed, as an entry keeps it.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
enum Seen {
    Read(PathBuf, Option<[u8; 32]>),
    /// A contract, by the digest of what the entry's seed reaches of it.
    Contract(PathBuf, [u8; 32]),
    Exists(PathBuf, bool),
    Listing(PathBuf, String, Option<Vec<PathBuf>>),
}

/// What an entry holds ahead of its module.
#[derive(Deserialize, Serialize)]
struct Header {
    seen: Vec<Seen>,
    /// The names the unit's sources and the other files it read spell, which
    /// its reach into the contracts starts from.
    seed: Vec<String>,
    warnings: String,
    /// The phase, by its `units`/`parse`/`elaborate`/`lower` index, that
    /// rejected the units, with its rendered diagnostics. No module follows.
    rejected: Option<(u8, String)>,
}

const PHASES: [DefinitionPhase; 4] = [
    DefinitionPhase::Units,
    DefinitionPhase::Parse,
    DefinitionPhase::Elaborate,
    DefinitionPhase::Lower,
];

pub(crate) struct DiskModuleCache {
    directory: PathBuf,
    /// The target and profile, which every slot of this run shares.
    profile: String,
    /// The release, definition and frontend, which every key of this run
    /// shares.
    toolchain: String,
    /// The diagnostics the phases of each unit the cache answered kept when
    /// they ran.
    warnings: Vec<String>,
    /// The entries being written.
    pending: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for DiskModuleCache {
    fn drop(&mut self) {
        for writing in self.pending.drain(..) {
            let _ = writing.join();
        }
    }
}

/// Driver flags whose run writes output while a unit elaborates, which a cached
/// `lower` answer would skip: a language's `kira doc` writes its documentation
/// from the elaborate phase, so a repeat run answered from the cache printed
/// nothing. A run with one of these set neither answers from the cache nor
/// stores to it.
const OUTPUT_FLAGS: &[&str] = &["doc"];

/// Whether the run sets a flag in `OUTPUT_FLAGS` to true.
fn writes_output(invocation: &klf_defsvc::driver_cli::Invocation) -> bool {
    invocation.flag_overrides.iter().any(|(name, value)| {
        OUTPUT_FLAGS.contains(&name.as_str()) && matches!(value, klf_services::DriverFlagValue::Boolean(true))
    })
}

impl DiskModuleCache {
    /// The cache for this run, or `None` when there is no cache directory.
    pub(crate) fn open(
        metadata: &FrontendMetadata,
        invocation: &klf_defsvc::driver_cli::Invocation,
        triple: &str,
        std_root: &str,
    ) -> Option<Self> {
        if writes_output(invocation) {
            return None;
        }
        let root = klf_target::cache_root()?;
        let directory = root.join("queries");
        let mut profile = format!(
            "target {triple}\nemit {}\nstd {std_root}\n",
            invocation.emit
        );
        let mut overrides: Vec<String> = invocation
            .flag_overrides
            .iter()
            .map(|(name, value)| format!("flag {name} {value:?}"))
            .collect();
        overrides.sort();
        for line in overrides {
            let _ = writeln!(profile, "{line}");
        }
        for (glob, mode) in &invocation.file_mode_globs {
            let _ = writeln!(profile, "mode {glob} {mode}");
        }
        let toolchain = format!(
            "klf {}\ndefinition {}\nfrontend {}\n",
            env!("CARGO_PKG_VERSION"),
            metadata.definition,
            frontend_identity(),
        );
        klf_defsvc::keep_subqueries(root.join("subqueries"), toolchain.clone());
        Some(Self {
            directory,
            profile,
            toolchain,
            warnings: Vec::new(),
            pending: Vec::new(),
        })
    }

    /// The release, definition, frontend, target and profile every key of
    /// this run shares: what a whole program built in it is keyed on besides
    /// its inputs.
    pub(crate) fn build_key(&self) -> String {
        format!("{}{}", self.toolchain, self.profile)
    }

    /// The diagnostics the phases kept: from the entries of the units the
    /// cache answered, and from the session for the units that ran.
    pub(crate) fn kept_diagnostics(&self, session: &Session) -> String {
        let ran = render_kept_diagnostics(session);
        self.warnings
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(ran.as_str()))
            .filter(|warnings| !warnings.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The slot directory of `unit` and the entry its key names. The key
    /// holds the unit's own files and the program's file names: the program
    /// decides which packages the parse reads, so the same unit under another
    /// program is another answer. Whatever else its phases read is checked
    /// against what the entry saw, so the other units' sources, which this
    /// one may never read, leave its entry alone.
    fn entry_path(
        &self,
        session: &Session,
        unit: &DefinitionUnit,
        program: &[FileId],
    ) -> Option<PathBuf> {
        let mut slot = self.profile.clone();
        slot.push_str("query lower\nunit");
        for file in unit.files() {
            let _ = write!(slot, " {}", session.file(*file).name());
        }
        slot.push('\n');
        let mut key = format!("{}{slot}", self.toolchain);
        for file in unit.files() {
            let file = session.file(*file);
            let _ = writeln!(
                key,
                "source {} {}",
                file.name(),
                hex(&sha256(file.text().as_bytes()))
            );
        }
        for file in program {
            let _ = writeln!(key, "program {}", session.file(*file).name());
        }
        Some(
            self.directory
                .join(hex(&sha256(slot.as_bytes())))
                .join(format!("{}.entry", hex(&sha256(key.as_bytes())))),
        )
    }
}

impl ModuleCache for DiskModuleCache {
    fn stored(&self, session: &Session, unit: &DefinitionUnit, program: &[FileId]) -> Option<Stored> {
        let path = self.entry_path(session, unit, program)?;
        if !path.is_file() {
            return None;
        }
        let overlay = session.overlay();
        Some(Box::new(move || {
            let bytes = std::fs::read(&path).ok()?;
            let (header, module): (Header, &[u8]) = postcard::take_from_bytes(&bytes).ok()?;
            let held = in_parallel(&header.seen, |seen| holds(seen, &overlay))?;
            if !held.into_iter().all(|held| held) || !contracts_hold(&header, &overlay) {
                return None;
            }
            let start = bytes.len() - module.len();
            let decode: Decode = Box::new(move || {
                let answer = match header.rejected {
                    Some((phase, message)) => Err((*PHASES.get(usize::from(phase))?, message)),
                    None => Ok(klf_kir::kirb::decode(&bytes[start..])?),
                };
                Some(Kept {
                    answer,
                    warnings: header.warnings,
                })
            });
            Some(decode)
        }))
    }

    fn answered(&mut self, warnings: String) {
        self.warnings.push(warnings);
    }

    fn store(
        &mut self,
        session: &Session,
        unit: &DefinitionUnit,
        program: &[FileId],
        trace: &UnitTrace,
        answer: Result<&Module, (DefinitionPhase, &str)>,
    ) {
        let Some(path) = self.entry_path(session, unit, program) else {
            return;
        };
        // A phase stopped by a cancellation, trap or panic did not answer.
        if answer.is_err() && crate::interrupt_pending() {
            return;
        }
        let observations = &session.observations()[trace.observations.clone()];
        let contract = |path: &PathBuf| path.extension().is_some_and(|extension| extension == "ridl");
        let seed = crate::contract_reach::seed(
            unit.files()
                .iter()
                .map(|file| session.file(*file).text())
                .chain(observations.iter().filter_map(|observation| match observation {
                    Observation::Read {
                        path,
                        text: Some(text),
                    } if !contract(path) => Some(&**text),
                    _ => None,
                })),
        );
        let mut contracts: Vec<(&PathBuf, &str)> = Vec::new();
        for observation in observations {
            if let Observation::Read {
                path,
                text: Some(text),
            } = observation
                && contract(path)
                && !contracts.iter().any(|(known, _)| *known == path)
            {
                contracts.push((path, text));
            }
        }
        let digests = crate::contract_reach::reached(
            &seed,
            &contracts.iter().map(|(_, text)| *text).collect::<Vec<_>>(),
        );
        let mut kept = HashSet::new();
        let seen: Vec<Seen> = observations
            .iter()
            .map(|observation| match observation {
                Observation::Read {
                    path,
                    text: Some(_),
                } if contract(path) => {
                    let at = contracts
                        .iter()
                        .position(|(known, _)| *known == path)
                        .unwrap_or_default();
                    Seen::Contract(path.clone(), digests[at])
                }
                Observation::Read { path, text } => Seen::Read(
                    path.clone(),
                    text.as_deref().map(|text| sha256(text.as_bytes())),
                ),
                Observation::Exists { path, found } => Seen::Exists(path.clone(), *found),
                Observation::Listing {
                    directory,
                    extension,
                    paths,
                } => Seen::Listing(directory.clone(), extension.clone(), paths.clone()),
            })
            .filter(|seen| kept.insert(seen.clone()))
            .collect();
        let rejected = answer.err().and_then(|(phase, message)| {
            let index = PHASES.iter().position(|known| *known == phase)?;
            Some((u8::try_from(index).ok()?, message.to_owned()))
        });
        let header = Header {
            seen,
            seed: seed.into_iter().collect(),
            warnings: render_diagnostics(session, trace.diagnostics.clone()),
            rejected,
        };
        // Encoding and writing the entry take seconds for a large package;
        // they run beside the code generation that follows, and the cache
        // waits for them when it is dropped.
        let module = answer.ok().cloned();
        self.pending.push(std::thread::spawn(move || {
            let Ok(mut bytes) = postcard::to_stdvec(&header) else {
                return;
            };
            if let Some(module) = &module {
                let Some(module) = klf_kir::kirb::encode(module) else {
                    return;
                };
                bytes.extend_from_slice(&module);
            }
            write_entry(&path, &bytes);
        }));
    }
}

/// Writes `bytes` as the entry at `path` and drops the other entries of its
/// slot. A cache that cannot be written only costs the next build its time,
/// and a reader never sees an entry half written.
fn write_entry(path: &std::path::Path, bytes: &[u8]) {
    let Some(slot) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(slot).is_err() {
        return;
    }
    let staged = path.with_extension(format!("{}.partial", std::process::id()));
    if std::fs::write(&staged, bytes).is_err() || std::fs::rename(&staged, path).is_err() {
        let _ = std::fs::remove_file(&staged);
        return;
    }
    let Ok(entries) = std::fs::read_dir(slot) else {
        return;
    };
    for entry in entries.flatten() {
        let other = entry.path();
        if other != path
            && other
                .extension()
                .is_some_and(|extension| extension == "entry")
        {
            let _ = std::fs::remove_file(other);
        }
    }
}

/// `check` of every item, split across the machine's cores, in item order;
/// `None` when a check panicked.
fn in_parallel<T: Sync, R: Send>(items: &[T], check: impl Fn(&T) -> R + Sync) -> Option<Vec<R>> {
    let workers = std::thread::available_parallelism().map_or(1, usize::from);
    let chunk = items.len().div_ceil(workers).max(1);
    let check = &check;
    std::thread::scope(|scope| {
        let parts: Vec<_> = items
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(check).collect::<Vec<R>>()))
            .collect();
        let mut results = Vec::with_capacity(items.len());
        for part in parts {
            results.extend(part.join().ok()?);
        }
        Some(results)
    })
}

/// Whether what the entry's seed reaches of each contract it read is what it
/// reached then. The contracts are reached together, as one names the types
/// of another, so they are checked together.
fn contracts_hold(header: &Header, overlay: &HashMap<PathBuf, Arc<str>>) -> bool {
    let mut paths = Vec::new();
    let mut digests = Vec::new();
    for seen in &header.seen {
        if let Seen::Contract(path, digest) = seen {
            paths.push(path);
            digests.push(*digest);
        }
    }
    if paths.is_empty() {
        return true;
    }
    let mut texts = Vec::new();
    for path in paths {
        match klf_services::read_through(overlay, path) {
            Ok(text) => texts.push(text),
            Err(_) => return false,
        }
    }
    let seed = header.seed.iter().cloned().collect();
    crate::contract_reach::reached(&seed, &texts.iter().map(|text| &**text).collect::<Vec<_>>())
        == digests
}

fn holds(seen: &Seen, overlay: &HashMap<PathBuf, Arc<str>>) -> bool {
    match seen {
        Seen::Contract(..) => true,
        Seen::Read(path, hash) => {
            klf_services::read_through(overlay, path)
                .ok()
                .map(|text| sha256(text.as_bytes()))
                == *hash
        }
        Seen::Exists(path, found) => klf_defsvc::path_present(overlay, path) == *found,
        Seen::Listing(directory, extension, paths) => {
            klf_defsvc::listing(directory, extension) == *paths
        }
    }
}

/// This binary's size and modification time: a development toolchain keeps
/// its release number across changes to the code that lowers a program.
fn frontend_identity() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.metadata().ok())
        .map_or_else(String::new, |built| {
            let modified = built
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            format!("{modified} {}", built.len())
        })
}
