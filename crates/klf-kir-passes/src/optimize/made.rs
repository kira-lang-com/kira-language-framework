//! What a run made of a function. With a memo, a function an earlier build
//! made alike stays in the memo until something reads its body: a caller
//! inlining it, the next run, or the module at the end.

use super::graph::calls_of;
use super::inline;
use super::memo::{self, Memo, Optimized};
use klf_core::Name;
use klf_kir::Function;
use std::sync::OnceLock;

pub(super) struct Made {
    body: OnceLock<Function>,
    /// The digest the body is kept under in a memo.
    pub(super) digest: [u8; 32],
    /// Whether the last round of every pass changed nothing.
    pub(super) at_rest: bool,
    pub(super) inlinable: bool,
    /// The functions the body calls directly.
    pub(super) calls: Vec<Name>,
}

impl Made {
    pub(super) fn here(function: Function, at_rest: bool, digest: [u8; 32]) -> Self {
        Self {
            inlinable: inline::inlinable(&function),
            calls: calls_of(&function),
            body: OnceLock::from(function),
            digest,
            at_rest,
        }
    }

    pub(super) fn kept(entry: Optimized) -> Self {
        Self {
            body: OnceLock::new(),
            digest: entry.body,
            at_rest: entry.at_rest,
            inlinable: entry.inlinable,
            calls: entry.calls,
        }
    }

    /// The entry an earlier build keeps this under, offered `offered`.
    pub(super) fn entry(&self, offered: Vec<(Name, memo::Offer)>) -> Optimized {
        Optimized {
            offered,
            body: self.digest,
            at_rest: self.at_rest,
            inlinable: self.inlinable,
            calls: self.calls.clone(),
        }
    }

    pub(super) fn body(&self, memo: Option<&dyn Memo>) -> &Function {
        self.body.get_or_init(|| load(memo, &self.digest))
    }

    pub(super) fn into_body(self, memo: Option<&dyn Memo>) -> Function {
        let digest = self.digest;
        self.body
            .into_inner()
            .unwrap_or_else(|| load(memo, &digest))
    }
}

/// A body the memo named as kept. One it no longer has leaves the module
/// without a function, so it cannot be built.
fn load(memo: Option<&dyn Memo>, digest: &[u8; 32]) -> Function {
    memo.and_then(|memo| memo::body(memo, digest))
        .expect("the optimizer's memo lost a function body it holds; clear its cache")
}

/// The body of each function made, read from `memo` across the cores.
pub(super) fn bodies(made: Vec<Option<Made>>, memo: Option<&dyn Memo>) -> Vec<Option<Function>> {
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let chunk = made.len().div_ceil(workers).max(1);
    let mut made = made;
    let mut chunks = Vec::new();
    while !made.is_empty() {
        let rest = made.split_off(chunk.min(made.len()));
        chunks.push(std::mem::replace(&mut made, rest));
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|made| made.map(|made| made.into_body(memo)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}
