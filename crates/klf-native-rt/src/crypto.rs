//! Native ABI shims for the klf.crypto kernel services.
//!
//! A program may pass one buffer as both an input and the destination, so
//! inputs are copied before the destination is borrowed mutably.

use klf_host_crypto::{self as crypto, Failure};

use crate::net::{bytes, bytes_mut};

const INVALID: i64 = Failure::Invalid as i64;

fn owned(pointer: *const u8, length: i64) -> Option<Vec<u8>> {
    bytes(pointer, length).map(<[u8]>::to_vec)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_random(destination: *mut u8, count: i64) -> i64 {
    bytes_mut(destination, count).map_or(INVALID, crypto::random)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_digest(
    hash: i64,
    data: *const u8,
    count: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(data) = owned(data, count) else {
        return INVALID;
    };
    let Some(destination) = bytes_mut(destination, capacity) else {
        return INVALID;
    };
    crypto::digest(hash, &data, destination)
}

/// HMAC, HKDF-Extract and HKDF-Expand share one shape: a hash, two inputs
/// and a destination.
macro_rules! keyed {
    ($symbol:ident, $operation:path) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $symbol(
            hash: i64,
            first: *const u8,
            first_count: i64,
            second: *const u8,
            second_count: i64,
            destination: *mut u8,
            capacity: i64,
        ) -> i64 {
            let (Some(first), Some(second)) =
                (owned(first, first_count), owned(second, second_count))
            else {
                return INVALID;
            };
            let Some(destination) = bytes_mut(destination, capacity) else {
                return INVALID;
            };
            $operation(hash, &first, &second, destination)
        }
    };
}

keyed!(klfsvc_crypto_hmac, crypto::hmac);
keyed!(klfsvc_crypto_hkdf_extract, crypto::hkdf_extract);
keyed!(klfsvc_crypto_hkdf_expand, crypto::hkdf_expand);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_aead_seal(
    suite: i64,
    key: *const u8,
    key_count: i64,
    nonce: *const u8,
    nonce_count: i64,
    aad: *const u8,
    aad_count: i64,
    buffer: *mut u8,
    count: i64,
    capacity: i64,
) -> i64 {
    let (Some(key), Some(nonce), Some(aad)) = (
        owned(key, key_count),
        owned(nonce, nonce_count),
        owned(aad, aad_count),
    ) else {
        return INVALID;
    };
    let Ok(count) = usize::try_from(count) else {
        return INVALID;
    };
    let Some(buffer) = bytes_mut(buffer, capacity) else {
        return INVALID;
    };
    crypto::aead_seal(suite, &key, &nonce, &aad, buffer, count)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_aead_open(
    suite: i64,
    key: *const u8,
    key_count: i64,
    nonce: *const u8,
    nonce_count: i64,
    aad: *const u8,
    aad_count: i64,
    buffer: *mut u8,
    count: i64,
) -> i64 {
    let (Some(key), Some(nonce), Some(aad)) = (
        owned(key, key_count),
        owned(nonce, nonce_count),
        owned(aad, aad_count),
    ) else {
        return INVALID;
    };
    let Some(buffer) = bytes_mut(buffer, count) else {
        return INVALID;
    };
    crypto::aead_open(suite, &key, &nonce, &aad, buffer)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_header_mask(
    suite: i64,
    key: *const u8,
    key_count: i64,
    sample: *const u8,
    sample_count: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let (Some(key), Some(sample)) = (owned(key, key_count), owned(sample, sample_count)) else {
        return INVALID;
    };
    let Some(destination) = bytes_mut(destination, capacity) else {
        return INVALID;
    };
    crypto::header_mask(suite, &key, &sample, destination)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_agreement_create(
    group: i64,
    public: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(public, capacity).map_or(INVALID, |public| crypto::agreement_create(group, public))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_agreement_finish(
    handle: i64,
    peer: *const u8,
    peer_count: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(peer) = owned(peer, peer_count) else {
        return INVALID;
    };
    let Some(destination) = bytes_mut(destination, capacity) else {
        return INVALID;
    };
    crypto::agreement_finish(handle, &peer, destination)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_verify(
    scheme: i64,
    certificate: *const u8,
    certificate_count: i64,
    message: *const u8,
    message_count: i64,
    signature: *const u8,
    signature_count: i64,
) -> i64 {
    let (Some(certificate), Some(message), Some(signature)) = (
        bytes(certificate, certificate_count),
        bytes(message, message_count),
        bytes(signature, signature_count),
    ) else {
        return INVALID;
    };
    crypto::verify(scheme, certificate, message, signature)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_key_load(der: *const u8, count: i64) -> i64 {
    bytes(der, count).map_or(INVALID, crypto::key_load)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_sign(
    handle: i64,
    scheme: i64,
    message: *const u8,
    count: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let Some(message) = owned(message, count) else {
        return INVALID;
    };
    let Some(destination) = bytes_mut(destination, capacity) else {
        return INVALID;
    };
    crypto::sign(handle, scheme, &message, destination)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_certificate_verify(
    chain: *const u8,
    chain_count: i64,
    name: *const u8,
    name_count: i64,
    seconds: i64,
    anchors: *const u8,
    anchors_count: i64,
) -> i64 {
    let (Some(chain), Some(name), Some(anchors)) = (
        bytes(chain, chain_count),
        bytes(name, name_count),
        bytes(anchors, anchors_count),
    ) else {
        return INVALID;
    };
    crypto::certificate_verify(chain, name, seconds, anchors)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_crypto_release(handle: i64) -> i64 {
    crypto::release(handle)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_digest_file(
    hash: i64,
    path: *const u8,
    length: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    let (Some(path), Some(destination)) = (owned(path, length), bytes_mut(destination, capacity))
    else {
        return INVALID;
    };
    crypto::digest_file(hash, &path, destination)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_key_generate(
    scheme: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(destination, capacity).map_or(INVALID, |destination| {
        crypto::key_generate(scheme, destination)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_crypto_key_public(
    handle: i64,
    destination: *mut u8,
    capacity: i64,
) -> i64 {
    bytes_mut(destination, capacity).map_or(INVALID, |destination| {
        crypto::key_public(handle, destination)
    })
}
