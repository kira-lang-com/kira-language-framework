//! `klf-rt`: the KLF runtime allocator and reference-counting-by-address.
//!
//! This is section 18 of the spec. The heap owns 64 KiB spans, each serving one
//! size class, plus dedicated runs for large allocations. Reference counts live
//! in per-slot metadata inside the allocator, not in the object, so counting is
//! by address: any interior pointer resolves to the allocation that holds it.
//!
//! The crate is systems code and uses `unsafe`. It builds for the host and for
//! `wasm32-unknown-unknown`, so the allocator logic avoids filesystem, process
//! and thread APIs. `std::alloc` backs the span memory.
//!
//! Construct one [`Heap`] per VM instance; there is no global mutable state.
// Android's thread locals are emulated, and the lint misreads the
// `thread_local!` expansion there as a non-`const` initializer.
#![cfg_attr(target_os = "android", allow(clippy::missing_const_for_thread_local))]
// The public API deliberately takes raw pointers by safe signature: the VM hands
// us object addresses and we resolve them through the page map before any access.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod decimal;
mod heap;
pub mod math;
mod meta;
mod pagemap;
mod service_memory;
mod sizeclass;
mod span;
mod valgrind;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_threads;

pub use heap::{DestroyFn, ExitReport, Heap};
pub use service_memory::{MAX_SERVICE_BUFFER_BYTES, ServiceMemory, copy_out};
