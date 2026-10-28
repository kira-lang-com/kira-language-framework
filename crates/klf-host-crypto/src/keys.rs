//! Private keys the program never reads: ephemeral key agreement and long-term
//! signing keys, held behind handles.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey};
use ring::rand::SystemRandom;
use ring::signature::{self, EcdsaKeyPair, Ed25519KeyPair, RsaKeyPair};

use crate::{Failure, deliver, group, length, scheme, word};

enum Key {
    // Taken when the agreement finishes, so a key agrees once.
    Agreement(Mutex<Option<EphemeralPrivateKey>>),
    Ed25519(Ed25519KeyPair),
    Ecdsa(EcdsaKeyPair, i64),
    Rsa(RsaKeyPair),
}

static KEYS: RwLock<Vec<Option<Arc<Key>>>> = RwLock::new(Vec::new());

fn insert(key: Key) -> i64 {
    let mut keys = KEYS.write().unwrap_or_else(PoisonError::into_inner);
    let entry = Some(Arc::new(key));
    let index = if let Some(free) = keys.iter().position(Option::is_none) {
        keys[free] = entry;
        free
    } else {
        keys.push(entry);
        keys.len() - 1
    };
    length(index + 1)
}

fn index(handle: i64) -> Result<usize, Failure> {
    usize::try_from(handle - 1).map_err(|_| Failure::InvalidHandle)
}

fn get(handle: i64) -> Result<Arc<Key>, Failure> {
    let keys = KEYS.read().unwrap_or_else(PoisonError::into_inner);
    keys.get(index(handle)?)
        .and_then(Option::clone)
        .ok_or(Failure::InvalidHandle)
}

/// Releases a key handle.
#[must_use]
pub fn release(handle: i64) -> i64 {
    word((|| {
        let mut keys = KEYS.write().unwrap_or_else(PoisonError::into_inner);
        let slot = keys.get_mut(index(handle)?).ok_or(Failure::InvalidHandle)?;
        slot.take().ok_or(Failure::InvalidHandle)?;
        Ok(1)
    })())
}

fn agreement_algorithm(code: i64) -> Result<&'static agreement::Algorithm, Failure> {
    match code {
        group::X25519 => Ok(&agreement::X25519),
        group::SECP256R1 => Ok(&agreement::ECDH_P256),
        _ => Err(Failure::Unsupported),
    }
}

/// Makes an ephemeral key pair in `group`, writes its public key to `public`
/// as the key share carries it (RFC 8446 section 4.2.8.2), and answers the
/// private key's handle.
#[must_use]
pub fn agreement_create(code: i64, public: &mut [u8]) -> i64 {
    word((|| {
        let algorithm = agreement_algorithm(code)?;
        let private = EphemeralPrivateKey::generate(algorithm, &SystemRandom::new())
            .map_err(|_| Failure::Invalid)?;
        let shared = private.compute_public_key().map_err(|_| Failure::Invalid)?;
        deliver(shared.as_ref(), public)?;
        Ok(insert(Key::Agreement(Mutex::new(Some(private)))))
    })())
}

/// Agrees with `peer`'s public key, writes the shared secret to
/// `destination` and answers its length. The handle is released whatever the
/// outcome, so the private key is used once.
#[must_use]
pub fn agreement_finish(handle: i64, peer: &[u8], destination: &mut [u8]) -> i64 {
    word((|| {
        let key = get(handle)?;
        let Key::Agreement(held) = &*key else {
            return Err(Failure::InvalidHandle);
        };
        let private = held.lock().unwrap_or_else(PoisonError::into_inner).take();
        let _ = release(handle);
        let private = private.ok_or(Failure::InvalidHandle)?;
        let peer = UnparsedPublicKey::new(private.algorithm(), peer);
        agreement::agree_ephemeral(private, &peer, |secret| deliver(secret, destination))
            .map_err(|_| Failure::Invalid)?
    })())
}

/// Loads a PKCS#8 private key in DER: Ed25519, ECDSA P-256 or P-384, or RSA.
/// Answers a signing-key handle.
#[must_use]
pub fn key_load(der: &[u8]) -> i64 {
    word((|| {
        if let Ok(pair) = Ed25519KeyPair::from_pkcs8_maybe_unchecked(der) {
            return Ok(insert(Key::Ed25519(pair)));
        }
        let random = SystemRandom::new();
        for (algorithm, code) in [
            (
                &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                scheme::ECDSA_SECP256R1_SHA256,
            ),
            (
                &signature::ECDSA_P384_SHA384_ASN1_SIGNING,
                scheme::ECDSA_SECP384R1_SHA384,
            ),
        ] {
            if let Ok(pair) = EcdsaKeyPair::from_pkcs8(algorithm, der, &random) {
                return Ok(insert(Key::Ecdsa(pair, code)));
            }
        }
        let pair = RsaKeyPair::from_pkcs8(der).map_err(|_| Failure::Invalid)?;
        Ok(insert(Key::Rsa(pair)))
    })())
}

/// Signs `message` with a key handle under `scheme`, which must be one the
/// key can make; writes the signature and answers its length.
#[must_use]
pub fn sign(handle: i64, code: i64, message: &[u8], destination: &mut [u8]) -> i64 {
    word((|| {
        let key = get(handle)?;
        let random = SystemRandom::new();
        match (&*key, code) {
            (Key::Ed25519(pair), scheme::ED25519) => {
                deliver(pair.sign(message).as_ref(), destination)
            }
            (Key::Ecdsa(pair, made), _) if *made == code => {
                let signed = pair.sign(&random, message).map_err(|_| Failure::Invalid)?;
                deliver(signed.as_ref(), destination)
            }
            (Key::Rsa(pair), scheme::RSA_PSS_RSAE_SHA256 | scheme::RSA_PSS_RSAE_SHA384) => {
                let padding: &'static dyn signature::RsaEncoding =
                    if code == scheme::RSA_PSS_RSAE_SHA256 {
                        &signature::RSA_PSS_SHA256
                    } else {
                        &signature::RSA_PSS_SHA384
                    };
                let size = pair.public().modulus_len();
                let room = destination.get_mut(..size).ok_or(Failure::TooSmall)?;
                pair.sign(padding, &random, message, room)
                    .map_err(|_| Failure::Invalid)?;
                Ok(length(size))
            }
            (Key::Agreement(_), _) => Err(Failure::InvalidHandle),
            _ => Err(Failure::Unsupported),
        }
    })())
}

/// Makes a new private key for `scheme` and writes it as PKCS#8 DER, which
/// `key_load` reads back; answers its length. Ed25519 only.
#[must_use]
pub fn key_generate(code: i64, destination: &mut [u8]) -> i64 {
    word((|| {
        if code != scheme::ED25519 {
            return Err(Failure::Unsupported);
        }
        let document =
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| Failure::Invalid)?;
        deliver(document.as_ref(), destination)
    })())
}

/// Writes the public key of a key handle, raw as its scheme encodes one
/// (32 bytes for Ed25519, an uncompressed point for ECDSA, PKCS#1 DER for
/// RSA); answers its length.
#[must_use]
pub fn key_public(handle: i64, destination: &mut [u8]) -> i64 {
    use ring::signature::KeyPair;
    word((|| match &*get(handle)? {
        Key::Ed25519(pair) => deliver(pair.public_key().as_ref(), destination),
        Key::Ecdsa(pair, _) => deliver(pair.public_key().as_ref(), destination),
        Key::Rsa(pair) => deliver(pair.public().as_ref(), destination),
        Key::Agreement(_) => Err(Failure::InvalidHandle),
    })())
}
