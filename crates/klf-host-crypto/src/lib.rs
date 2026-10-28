//! The `klf.crypto` kernel services: vetted primitives and certificate path
//! validation, for protocols the hosted language writes itself.
//!
//! Everything a protocol builds from these, a TLS 1.3 handshake, its key
//! schedule, QUIC packet protection, is the hosted language's. What stays
//! here is what a subtle bug turns into a silent, remote break: constant-time
//! arithmetic (`ring`) and X.509 chain building (`rustls-webpki`). A private
//! key the program never reads stays behind a handle.
//!
//! Algorithms are named by their TLS registry code points (RFC 8446 section
//! 4.2.3 and 4.2.7, and the cipher suites of section B.4), so no second
//! numbering is invented. A failure is a negative word from [`Failure`].

mod certificates;
mod keys;
mod primitives;

pub use certificates::{certificate_verify, verify};
pub use keys::{
    agreement_create, agreement_finish, key_generate, key_load, key_public, release, sign,
};
pub use primitives::{
    aead_open, aead_seal, digest, digest_file, header_mask, hkdf_expand, hkdf_extract, hmac, random,
};

/// Why a cryptographic operation failed, as the negative word it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Failure {
    /// No algorithm has this code point here.
    Unsupported = -1,
    /// A key, nonce, sample or encoding is malformed or the wrong length.
    Invalid = -2,
    /// Sealed data or a signature did not verify.
    Rejected = -3,
    InvalidHandle = -4,
    /// The destination is smaller than the answer.
    TooSmall = -5,
    /// No trust anchor issued the chain.
    UnknownIssuer = -6,
    /// A certificate is expired or not yet valid at the time given.
    Expired = -7,
    /// The end-entity certificate does not name the server.
    WrongName = -8,
    /// Any other reason the chain is not acceptable.
    BadCertificate = -9,
}

/// Hash code points (RFC 5246 section 7.4.1.4.1), which HKDF and HMAC name.
pub mod hash {
    pub const SHA256: i64 = 4;
    pub const SHA384: i64 = 5;
}

/// TLS 1.3 cipher suites (RFC 8446 section B.4), which name an AEAD.
pub mod suite {
    pub const AES_128_GCM_SHA256: i64 = 0x1301;
    pub const AES_256_GCM_SHA384: i64 = 0x1302;
    pub const CHACHA20_POLY1305_SHA256: i64 = 0x1303;
}

/// Key-agreement groups (RFC 8446 section 4.2.7).
pub mod group {
    pub const SECP256R1: i64 = 0x0017;
    pub const X25519: i64 = 0x001d;
}

/// Signature schemes (RFC 8446 section 4.2.3).
pub mod scheme {
    pub const ECDSA_SECP256R1_SHA256: i64 = 0x0403;
    pub const ECDSA_SECP384R1_SHA384: i64 = 0x0503;
    pub const RSA_PSS_RSAE_SHA256: i64 = 0x0804;
    pub const RSA_PSS_RSAE_SHA384: i64 = 0x0805;
    pub const ED25519: i64 = 0x0807;
}

fn word(result: Result<i64, Failure>) -> i64 {
    result.unwrap_or_else(|failure| failure as i64)
}

fn length(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Copies `answer` into the front of `destination` and answers its length.
fn deliver(answer: &[u8], destination: &mut [u8]) -> Result<i64, Failure> {
    let room = destination
        .get_mut(..answer.len())
        .ok_or(Failure::TooSmall)?;
    room.copy_from_slice(answer);
    Ok(length(answer.len()))
}
