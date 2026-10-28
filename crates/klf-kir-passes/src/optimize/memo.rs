//! What one build of a module optimized, kept for the next. A function's
//! optimization is named by everything it reads: its body, the run it is in,
//! and the module's signatures. What it read of other functions, the bodies
//! it was offered to inline, is recorded with it and compared before the
//! entry is used, so an entry stands only while those bodies are the same.

use klf_core::Name;
use klf_kir::{Function, Module};

/// Where optimized functions are kept between builds. The optimizer names
/// an entry by `digest` and stores whatever bytes it likes under it. Every
/// entry a build reads it puts or reuses, so a store can drop the others.
pub trait Memo: Sync {
    /// A digest of `bytes` an entry is named by.
    fn digest(&self, bytes: &[u8]) -> [u8; 32];
    /// The entry named `key`, from an earlier build.
    fn get(&self, key: &[u8; 32]) -> Option<Vec<u8>>;
    /// Keeps `entry` under `key` for the next build.
    fn put(&self, key: [u8; 32], entry: Vec<u8>);
    /// Keeps the entry named `key`, which this build used as it was.
    fn reuse(&self, key: &[u8; 32]);
}

/// What a function was offered of a callee: none, or the digest of the body
/// and whether the callee is on a cycle.
pub(super) type Offer = Option<([u8; 32], bool)>;

/// A function as a run optimized it, by the digest its body is kept under,
/// with what it was offered and what its callers and the next run read
/// of it besides its body.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Optimized {
    pub(super) offered: Vec<(Name, Offer)>,
    pub(super) body: [u8; 32],
    pub(super) at_rest: bool,
    pub(super) inlinable: bool,
    pub(super) calls: Vec<Name>,
}

/// The digest of `function`, with `placeless` none of the places its parts
/// were written: those move whenever text above them changes, and a backend
/// that reads none of them makes the same code either way.
pub(super) fn digest_of(memo: &dyn Memo, function: &Function, placeless: bool) -> [u8; 32] {
    if !placeless {
        return memo.digest(&postcard::to_stdvec(function).unwrap_or_default());
    }
    let mut moved = function.clone();
    moved.span = placeless_span(moved.span);
    for parameter in &mut moved.parameters {
        parameter.span = placeless_span(parameter.span);
    }
    for block in &mut moved.blocks {
        block.span = placeless_span(block.span);
        for parameter in &mut block.parameters {
            parameter.span = placeless_span(parameter.span);
        }
        for instruction in &mut block.instructions {
            instruction.span = placeless_span(instruction.span);
        }
    }
    memo.digest(&postcard::to_stdvec(&moved).unwrap_or_default())
}

/// Keeps `function` under its digest, which it answers. A body two entries
/// share is kept once.
pub(super) fn keep_body(memo: &dyn Memo, function: &Function, placeless: bool) -> [u8; 32] {
    let bytes = postcard::to_stdvec(function).unwrap_or_default();
    let digest = if placeless {
        digest_of(memo, function, true)
    } else {
        memo.digest(&bytes)
    };
    memo.put(digest, bytes);
    digest
}

/// The body kept under `digest`.
pub(super) fn body(memo: &dyn Memo, digest: &[u8; 32]) -> Option<Function> {
    postcard::from_bytes(&memo.get(digest)?).ok()
}

pub(super) fn entry<T: serde::de::DeserializeOwned>(memo: &dyn Memo, key: &[u8; 32]) -> Option<T> {
    postcard::from_bytes(&memo.get(key)?).ok()
}

pub(super) fn keep<T: serde::Serialize>(memo: &dyn Memo, key: [u8; 32], entry: &T) {
    memo.put(key, postcard::to_stdvec(entry).unwrap_or_default());
}

/// The digest of what every function of a run reads besides its own body:
/// the module's signatures, types and tables, without the places they were
/// written, which move whenever a source file above them changes.
pub(super) fn shell_digest(memo: &dyn Memo, shell: &Module, run: &str) -> [u8; 32] {
    let mut placeless = shell.clone();
    for function in &mut placeless.functions {
        function.span = placeless_span(function.span);
        for parameter in &mut function.parameters {
            parameter.span = placeless_span(parameter.span);
        }
    }
    for declaration in &mut placeless.types {
        declaration.span = placeless_span(declaration.span);
    }
    let mut bytes = postcard::to_stdvec(&placeless).unwrap_or_default();
    bytes.extend_from_slice(run.as_bytes());
    memo.digest(&bytes)
}

fn placeless_span(span: klf_source::Span) -> klf_source::Span {
    klf_source::Span::empty(span.file, 0)
}

/// The name of a function's entry: the run's digest, its body's, and the
/// facts about it the run decides by.
pub(super) fn key(memo: &dyn Memo, run: &[u8; 32], body: &[u8; 32], facts: &[bool]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(64 + facts.len());
    bytes.extend_from_slice(run);
    bytes.extend_from_slice(body);
    bytes.extend(facts.iter().map(|&fact| u8::from(fact)));
    memo.digest(&bytes)
}
