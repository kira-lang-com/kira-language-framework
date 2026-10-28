//! `klf-live`: the Kira Live Runtime's wire and its engine seam (section 17.5).
//!
//! A client compiles an app and pushes its KIR over a TCP connection; a device
//! running the app lowers that KIR, installs it as a new code version and
//! keeps every value the running program already holds. The device answers
//! each push with an ack or a nack, so a client knows what actually landed.
//!
//! The pieces are the [`wire`] frame codec, the [`engine`] seam every engine
//! implements, the [`transport`] framing over a socket, the [`client`] that
//! pushes, and the [`Device`] that drives an engine through the exchange. The
//! VM body of the seam is here; the native and hybrid bodies re-sign and
//! launch binaries against the same signatures.

pub mod artifact;
pub mod client;
pub mod engine;
pub mod transport;
pub mod wire;

pub use artifact::{Artifact, Bundle, RunOutcome, RunRequest};
pub use client::{Client, Pushed};
pub use engine::{Device, Engine, EngineError, Refusal, Signer, VmEngine};
pub use transport::Connection;
pub use wire::{Ack, Frame, Hello, Nack, Push, Reason, VERSION};
