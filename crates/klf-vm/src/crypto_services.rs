//! The klf.crypto kernel services against VM memory.

use klf_host_crypto::{self as crypto, Failure};
use klf_rt::ServiceMemory;

use crate::Capabilities;

const INVALID: u64 = (Failure::Invalid as i64).cast_unsigned();

fn input(memory: &dyn ServiceMemory, address: u64, length: u64) -> Option<Vec<u8>> {
    let length = usize::try_from(length).ok()?;
    if length > klf_rt::MAX_SERVICE_BUFFER_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    if length != 0 && !memory.read_buffer(address, &mut bytes) {
        return None;
    }
    Some(bytes)
}

fn output(capacity: u64) -> Option<Vec<u8>> {
    let capacity = usize::try_from(capacity).ok()?;
    (capacity <= klf_rt::MAX_SERVICE_BUFFER_BYTES).then(|| vec![0; capacity])
}

/// Copies the first `answer` bytes of `bytes` back when the answer is a
/// length, and answers it.
fn copy_back(memory: &mut dyn ServiceMemory, address: u64, bytes: &[u8], answer: i64) -> u64 {
    if let Ok(written) = usize::try_from(answer)
        && written != 0
        && !memory.write_buffer(address, &bytes[..written.min(bytes.len())])
    {
        return INVALID;
    }
    answer.cast_unsigned()
}

pub(crate) fn register(capabilities: &mut Capabilities) {
    register_primitives(capabilities);
    register_keys(capabilities);
}

fn register_primitives(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_crypto_random", |arguments, memory| {
        let [destination, count] = arguments else {
            return INVALID;
        };
        let Some(mut bytes) = output(*count) else {
            return INVALID;
        };
        let answer = crypto::random(&mut bytes);
        if answer == 1 && !bytes.is_empty() && !memory.write_buffer(*destination, &bytes) {
            return INVALID;
        }
        answer.cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_crypto_digest", |arguments, memory| {
        let [hash, data, count, destination, capacity] = arguments else {
            return INVALID;
        };
        let (Some(data), Some(mut out)) = (input(memory, *data, *count), output(*capacity)) else {
            return INVALID;
        };
        let answer = crypto::digest(hash.cast_signed(), &data, &mut out);
        copy_back(memory, *destination, &out, answer)
    });
    for (symbol, operation) in [
        (
            "klfsvc_crypto_hmac",
            crypto::hmac as fn(i64, &[u8], &[u8], &mut [u8]) -> i64,
        ),
        ("klfsvc_crypto_hkdf_extract", crypto::hkdf_extract),
        ("klfsvc_crypto_hkdf_expand", crypto::hkdf_expand),
    ] {
        capabilities.register_with_memory(symbol, move |arguments, memory| {
            let [
                hash,
                first,
                first_count,
                second,
                second_count,
                destination,
                capacity,
            ] = arguments
            else {
                return INVALID;
            };
            let (Some(first), Some(second), Some(mut out)) = (
                input(memory, *first, *first_count),
                input(memory, *second, *second_count),
                output(*capacity),
            ) else {
                return INVALID;
            };
            let answer = operation(hash.cast_signed(), &first, &second, &mut out);
            copy_back(memory, *destination, &out, answer)
        });
    }
    capabilities.register_with_memory("klfsvc_crypto_aead_seal", |arguments, memory| {
        let [
            suite,
            key,
            key_count,
            nonce,
            nonce_count,
            aad,
            aad_count,
            buffer,
            count,
            capacity,
        ] = arguments
        else {
            return INVALID;
        };
        let (Some(key), Some(nonce), Some(aad), Some(plain), Some(mut room), Ok(count)) = (
            input(memory, *key, *key_count),
            input(memory, *nonce, *nonce_count),
            input(memory, *aad, *aad_count),
            input(memory, *buffer, *count),
            output(*capacity),
            usize::try_from(*count),
        ) else {
            return INVALID;
        };
        if room.len() < count {
            return (Failure::TooSmall as i64).cast_unsigned();
        }
        room[..count].copy_from_slice(&plain);
        let answer = crypto::aead_seal(suite.cast_signed(), &key, &nonce, &aad, &mut room, count);
        copy_back(memory, *buffer, &room, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_aead_open", |arguments, memory| {
        let [
            suite,
            key,
            key_count,
            nonce,
            nonce_count,
            aad,
            aad_count,
            buffer,
            count,
        ] = arguments
        else {
            return INVALID;
        };
        let (Some(key), Some(nonce), Some(aad), Some(mut sealed)) = (
            input(memory, *key, *key_count),
            input(memory, *nonce, *nonce_count),
            input(memory, *aad, *aad_count),
            input(memory, *buffer, *count),
        ) else {
            return INVALID;
        };
        let answer = crypto::aead_open(suite.cast_signed(), &key, &nonce, &aad, &mut sealed);
        copy_back(memory, *buffer, &sealed, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_header_mask", |arguments, memory| {
        let [
            suite,
            key,
            key_count,
            sample,
            sample_count,
            destination,
            capacity,
        ] = arguments
        else {
            return INVALID;
        };
        let (Some(key), Some(sample), Some(mut out)) = (
            input(memory, *key, *key_count),
            input(memory, *sample, *sample_count),
            output(*capacity),
        ) else {
            return INVALID;
        };
        let answer = crypto::header_mask(suite.cast_signed(), &key, &sample, &mut out);
        copy_back(memory, *destination, &out, answer)
    });
}

fn register_keys(capabilities: &mut Capabilities) {
    capabilities.register_with_memory("klfsvc_crypto_key_generate", |arguments, memory| {
        let [scheme, destination, capacity] = arguments else {
            return INVALID;
        };
        let Some(mut bytes) = output(*capacity) else {
            return INVALID;
        };
        let answer = crypto::key_generate(scheme.cast_signed(), &mut bytes);
        copy_back(memory, *destination, &bytes, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_key_public", |arguments, memory| {
        let [handle, destination, capacity] = arguments else {
            return INVALID;
        };
        let Some(mut bytes) = output(*capacity) else {
            return INVALID;
        };
        let answer = crypto::key_public(handle.cast_signed(), &mut bytes);
        copy_back(memory, *destination, &bytes, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_digest_file", |arguments, memory| {
        let [hash, path, length, destination, capacity] = arguments else {
            return INVALID;
        };
        let (Some(path), Some(mut bytes)) = (input(memory, *path, *length), output(*capacity))
        else {
            return INVALID;
        };
        let answer = crypto::digest_file(hash.cast_signed(), &path, &mut bytes);
        copy_back(memory, *destination, &bytes, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_agreement_create", |arguments, memory| {
        let [group, public, capacity] = arguments else {
            return INVALID;
        };
        let Some(mut out) = output(*capacity) else {
            return INVALID;
        };
        let answer = crypto::agreement_create(group.cast_signed(), &mut out);
        // The answer is a handle; the public key filled the front of `out`.
        if answer > 0 && !out.is_empty() && !memory.write_buffer(*public, &out) {
            return INVALID;
        }
        answer.cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_crypto_agreement_finish", |arguments, memory| {
        let [handle, peer, peer_count, destination, capacity] = arguments else {
            return INVALID;
        };
        let (Some(peer), Some(mut out)) = (input(memory, *peer, *peer_count), output(*capacity))
        else {
            return INVALID;
        };
        let answer = crypto::agreement_finish(handle.cast_signed(), &peer, &mut out);
        copy_back(memory, *destination, &out, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_verify", |arguments, memory| {
        let [
            scheme,
            certificate,
            certificate_count,
            message,
            message_count,
            signature,
            signature_count,
        ] = arguments
        else {
            return INVALID;
        };
        let (Some(certificate), Some(message), Some(signature)) = (
            input(memory, *certificate, *certificate_count),
            input(memory, *message, *message_count),
            input(memory, *signature, *signature_count),
        ) else {
            return INVALID;
        };
        crypto::verify(scheme.cast_signed(), &certificate, &message, &signature).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_crypto_key_load", |arguments, memory| {
        let [der, count] = arguments else {
            return INVALID;
        };
        let Some(der) = input(memory, *der, *count) else {
            return INVALID;
        };
        crypto::key_load(&der).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_crypto_sign", |arguments, memory| {
        let [handle, scheme, message, count, destination, capacity] = arguments else {
            return INVALID;
        };
        let (Some(message), Some(mut out)) = (input(memory, *message, *count), output(*capacity))
        else {
            return INVALID;
        };
        let answer = crypto::sign(
            handle.cast_signed(),
            scheme.cast_signed(),
            &message,
            &mut out,
        );
        copy_back(memory, *destination, &out, answer)
    });
    capabilities.register_with_memory("klfsvc_crypto_certificate_verify", |arguments, memory| {
        let [
            chain,
            chain_count,
            name,
            name_count,
            seconds,
            anchors,
            anchors_count,
        ] = arguments
        else {
            return INVALID;
        };
        let (Some(chain), Some(name), Some(anchors)) = (
            input(memory, *chain, *chain_count),
            input(memory, *name, *name_count),
            input(memory, *anchors, *anchors_count),
        ) else {
            return INVALID;
        };
        crypto::certificate_verify(&chain, &name, seconds.cast_signed(), &anchors).cast_unsigned()
    });
    capabilities.register_with_memory("klfsvc_crypto_release", |arguments, _| {
        let [handle] = arguments else { return INVALID };
        crypto::release(handle.cast_signed()).cast_unsigned()
    });
}
