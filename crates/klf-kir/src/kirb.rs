//! `.kirb`: a module in the binary encoding private to this KLF release
//! (section 10). Only a toolchain of the same release reads it back, so the
//! encoding carries no version of its own; the cache key does.
//!
//! The bytes are a count of parts, then each part's length and bytes. The
//! first part is the module without its functions; each other part is a run of
//! its functions, in order. A part is its distinct names, then its items with
//! each name written as its index among them, so the parts encode and decode
//! on the machine's cores at once.

use crate::{Function, Module};
use klf_core::{Name, reading_name_table, writing_name_table};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Functions per part: enough that a part outweighs taking it, few enough
/// that the cores share a module evenly.
const FUNCTIONS_PER_PART: usize = 256;

/// The module's bytes, or `None` when a part could not be encoded.
#[must_use]
pub fn encode(module: &Module) -> Option<Vec<u8>> {
    let shell = Module {
        name: module.name.clone(),
        language: module.language.clone(),
        target: module.target.clone(),
        stage: module.stage,
        attributes: module.attributes.clone(),
        types: module.types.clone(),
        globals: module.globals.clone(),
        vtables: module.vtables.clone(),
        traits: module.traits.clone(),
        conformances: module.conformances.clone(),
        imports: module.imports.clone(),
        exports: module.exports.clone(),
        witnesses: module.witnesses.clone(),
        functions: Vec::new(),
    };
    let runs: Vec<&[Function]> = module.functions.chunks(FUNCTIONS_PER_PART).collect();
    let mut parts = vec![encode_part(&shell)?];
    for part in on_cores(&runs, |run| encode_part(*run))? {
        parts.push(part?);
    }
    let mut bytes = Vec::with_capacity(parts.iter().map(|part| part.len() + 8).sum::<usize>() + 8);
    bytes.extend_from_slice(&(parts.len() as u64).to_le_bytes());
    for part in &parts {
        bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
        bytes.extend_from_slice(part);
    }
    Some(bytes)
}

/// The module `bytes` encode, or `None` when they are not one.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Module> {
    let (count, mut rest) = take_length(bytes)?;
    let mut parts = Vec::with_capacity(count.min(rest.len()));
    for _ in 0..count {
        let (length, after) = take_length(rest)?;
        if after.len() < length {
            return None;
        }
        let (part, after) = after.split_at(length);
        parts.push(part);
        rest = after;
    }
    let (shell, runs) = parts.split_first()?;
    let mut module: Module = decode_part(shell)?;
    for run in on_cores(runs, |run| decode_part::<Vec<Function>>(run))? {
        module.functions.extend(run?);
    }
    Some(module)
}

fn encode_part<T: Serialize + ?Sized>(item: &T) -> Option<Vec<u8>> {
    let (body, names) = writing_name_table(|| postcard::to_stdvec(item).ok());
    let texts: Vec<&str> = names.iter().map(Name::as_str).collect();
    let mut bytes = postcard::to_stdvec(&texts).ok()?;
    bytes.extend_from_slice(&body?);
    Some(bytes)
}

fn decode_part<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    let (texts, body): (Vec<&str>, &[u8]) = postcard::take_from_bytes(bytes).ok()?;
    let names = texts.into_iter().map(Name::new).collect();
    reading_name_table(names, || postcard::from_bytes(body).ok())
}

fn take_length(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let (length, rest) = bytes.split_first_chunk::<8>()?;
    Some((usize::try_from(u64::from_le_bytes(*length)).ok()?, rest))
}

/// `work` on every item, the machine's cores taking items in turn, with the
/// results in item order; `None` when a thread panicked.
fn on_cores<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Option<Vec<R>> {
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(items.len());
    if threads <= 1 {
        return Some(items.iter().map(work).collect());
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let work = &work;
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut taken = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            break taken;
                        };
                        taken.push((index, work(item)));
                    }
                })
            })
            .collect();
        let mut done = Vec::with_capacity(items.len());
        for worker in workers {
            done.extend(worker.join().ok()?);
        }
        Some(done)
    })?;
    done.sort_by_key(|(index, _)| *index);
    Some(done.into_iter().map(|(_, result)| result).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_module_decodes_to_itself() {
        let text = include_str!("../../klf-kir-passes/tests/fixtures/karc-fibonacci.kir");
        let mut module = crate::parse_module(text).expect("the fixture parses");
        // Enough functions for several parts, each decoded on its own.
        let template = module.functions.clone();
        for copy in 0..600 {
            for function in &template {
                let mut function = function.clone();
                function.name = klf_core::Name::new(format!("{}_{copy}", function.name));
                module.functions.push(function);
            }
        }
        let bytes = super::encode(&module).expect("the module encodes");
        assert_eq!(super::decode(&bytes), Some(module));
        assert_eq!(super::decode(&bytes[..bytes.len() - 1]), None);
    }
}
