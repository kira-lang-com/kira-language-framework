//! Randomness, hashes, HMAC, HKDF, AEAD and QUIC header protection.

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey, quic};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest as hashing, hkdf, hmac as mac};

use crate::{Failure, deliver, hash, length, suite, word};

/// The tag every TLS 1.3 AEAD appends.
pub const TAG_LENGTH: usize = 16;
/// The bytes of a QUIC header-protection mask (RFC 9001 section 5.4.1).
pub const MASK_LENGTH: usize = 5;

/// Fills `destination` from the operating system's generator.
#[must_use]
pub fn random(destination: &mut [u8]) -> i64 {
    word(
        SystemRandom::new()
            .fill(destination)
            .map(|()| 1)
            .map_err(|_| Failure::Invalid),
    )
}

fn digest_algorithm(code: i64) -> Result<&'static hashing::Algorithm, Failure> {
    match code {
        hash::SHA256 => Ok(&hashing::SHA256),
        hash::SHA384 => Ok(&hashing::SHA384),
        _ => Err(Failure::Unsupported),
    }
}

fn mac_algorithm(code: i64) -> Result<mac::Algorithm, Failure> {
    match code {
        hash::SHA256 => Ok(mac::HMAC_SHA256),
        hash::SHA384 => Ok(mac::HMAC_SHA384),
        _ => Err(Failure::Unsupported),
    }
}

/// The hash of `data`, written to `destination`; answers its length.
#[must_use]
pub fn digest(code: i64, data: &[u8], destination: &mut [u8]) -> i64 {
    word(
        digest_algorithm(code)
            .and_then(|algorithm| deliver(hashing::digest(algorithm, data).as_ref(), destination)),
    )
}

#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "bytes name no path off Unix")]
fn file_path(bytes: &[u8]) -> Result<std::path::PathBuf, Failure> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::OsStr::from_bytes(bytes).into())
}

#[cfg(not(unix))]
fn file_path(bytes: &[u8]) -> Result<std::path::PathBuf, Failure> {
    std::str::from_utf8(bytes)
        .map(std::path::PathBuf::from)
        .map_err(|_| Failure::Invalid)
}

/// The hash of the file at `path`, read in pieces so it never sits in
/// memory whole, written to `destination`; answers its length.
#[must_use]
pub fn digest_file(code: i64, path: &[u8], destination: &mut [u8]) -> i64 {
    use std::io::Read;
    word((|| {
        let mut context = hashing::Context::new(digest_algorithm(code)?);
        let mut file = std::fs::File::open(file_path(path)?).map_err(|_| Failure::Invalid)?;
        let mut chunk = vec![0_u8; 1 << 20];
        loop {
            let read = file.read(&mut chunk).map_err(|_| Failure::Invalid)?;
            if read == 0 {
                break;
            }
            context.update(&chunk[..read]);
        }
        deliver(context.finish().as_ref(), destination)
    })())
}

/// HMAC of `data` under `key`.
#[must_use]
pub fn hmac(code: i64, key: &[u8], data: &[u8], destination: &mut [u8]) -> i64 {
    word((|| {
        let key = mac::Key::new(mac_algorithm(code)?, key);
        deliver(mac::sign(&key, data).as_ref(), destination)
    })())
}

/// HKDF-Extract (RFC 5869 section 2.2). It is HMAC with the salt as the key,
/// which is how the pseudorandom key comes out as bytes: TLS 1.3's key
/// schedule (RFC 8446 section 7.1) feeds it back into the next extract.
#[must_use]
pub fn hkdf_extract(code: i64, salt: &[u8], keying: &[u8], destination: &mut [u8]) -> i64 {
    hmac(code, salt, keying, destination)
}

struct Length(usize);

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF-Expand (RFC 5869 section 2.3) to exactly `destination.len()` bytes.
#[must_use]
pub fn hkdf_expand(code: i64, pseudorandom: &[u8], info: &[u8], destination: &mut [u8]) -> i64 {
    word((|| {
        let algorithm = match code {
            hash::SHA256 => hkdf::HKDF_SHA256,
            hash::SHA384 => hkdf::HKDF_SHA384,
            _ => return Err(Failure::Unsupported),
        };
        let wanted = algorithm.hmac_algorithm().digest_algorithm().output_len();
        if pseudorandom.len() < wanted {
            return Err(Failure::Invalid);
        }
        let key = hkdf::Prk::new_less_safe(algorithm, pseudorandom);
        let info = [info];
        let output = key
            .expand(&info, Length(destination.len()))
            .map_err(|_| Failure::Invalid)?;
        output.fill(destination).map_err(|_| Failure::Invalid)?;
        Ok(length(destination.len()))
    })())
}

fn aead_key(code: i64, key: &[u8]) -> Result<LessSafeKey, Failure> {
    let algorithm = match code {
        suite::AES_128_GCM_SHA256 => &aead::AES_128_GCM,
        suite::AES_256_GCM_SHA384 => &aead::AES_256_GCM,
        suite::CHACHA20_POLY1305_SHA256 => &aead::CHACHA20_POLY1305,
        _ => return Err(Failure::Unsupported),
    };
    Ok(LessSafeKey::new(
        UnboundKey::new(algorithm, key).map_err(|_| Failure::Invalid)?,
    ))
}

fn nonce(bytes: &[u8]) -> Result<Nonce, Failure> {
    Nonce::try_assume_unique_for_key(bytes).map_err(|_| Failure::Invalid)
}

/// Seals the first `count` bytes of `buffer` in place and writes the tag after
/// them; `buffer` is the whole room the caller has. Answers `count` plus the
/// tag's length.
#[must_use]
pub fn aead_seal(
    code: i64,
    key: &[u8],
    nonce_bytes: &[u8],
    aad: &[u8],
    buffer: &mut [u8],
    count: usize,
) -> i64 {
    word((|| {
        let key = aead_key(code, key)?;
        let sealed = count.checked_add(TAG_LENGTH).ok_or(Failure::TooSmall)?;
        if buffer.len() < sealed {
            return Err(Failure::TooSmall);
        }
        let (plain, rest) = buffer.split_at_mut(count);
        let tag = key
            .seal_in_place_separate_tag(nonce(nonce_bytes)?, Aad::from(aad), plain)
            .map_err(|_| Failure::Invalid)?;
        rest[..TAG_LENGTH].copy_from_slice(tag.as_ref());
        Ok(length(sealed))
    })())
}

/// Opens `buffer`, ciphertext then tag, in place; the plaintext is left at
/// its front. Answers the plaintext's length, or rejected when the tag does
/// not verify, in which case the buffer holds nothing to read.
#[must_use]
pub fn aead_open(code: i64, key: &[u8], nonce_bytes: &[u8], aad: &[u8], buffer: &mut [u8]) -> i64 {
    word((|| {
        let key = aead_key(code, key)?;
        if buffer.len() < TAG_LENGTH {
            return Err(Failure::Rejected);
        }
        let plain = key
            .open_in_place(nonce(nonce_bytes)?, Aad::from(aad), buffer)
            .map_err(|_| Failure::Rejected)?;
        Ok(length(plain.len()))
    })())
}

/// The header-protection mask of `sample` (RFC 9001 section 5.4): AES-ECB or
/// `ChaCha20` by the suite the packet keys come from.
#[must_use]
pub fn header_mask(code: i64, key: &[u8], sample: &[u8], destination: &mut [u8]) -> i64 {
    word((|| {
        let algorithm = match code {
            suite::AES_128_GCM_SHA256 => &quic::AES_128,
            suite::AES_256_GCM_SHA384 => &quic::AES_256,
            suite::CHACHA20_POLY1305_SHA256 => &quic::CHACHA20,
            _ => return Err(Failure::Unsupported),
        };
        let key = quic::HeaderProtectionKey::new(algorithm, key).map_err(|_| Failure::Invalid)?;
        let mask = key.new_mask(sample).map_err(|_| Failure::Invalid)?;
        deliver(&mask[..MASK_LENGTH], destination)
    })())
}
