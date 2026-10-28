use klf_host_crypto::{self as crypto, Failure, group, hash, scheme, suite};

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

/// TLS 1.3's `HkdfLabel` (RFC 8446 section 7.1) with an empty context.
fn label(name: &str, length: u16) -> Vec<u8> {
    let full = format!("tls13 {name}");
    let mut info = length.to_be_bytes().to_vec();
    info.push(u8::try_from(full.len()).unwrap());
    info.extend_from_slice(full.as_bytes());
    info.push(0);
    info
}

fn expand(prk: &[u8], name: &str, length: u16) -> Vec<u8> {
    let mut out = vec![0; usize::from(length)];
    assert_eq!(
        crypto::hkdf_expand(hash::SHA256, prk, &label(name, length), &mut out),
        i64::from(length)
    );
    out
}

#[test]
fn hkdf_matches_rfc_5869_case_1() {
    let mut prk = [0; 32];
    let salt = hex("000102030405060708090a0b0c");
    assert_eq!(
        crypto::hkdf_extract(hash::SHA256, &salt, &[0x0b; 22], &mut prk),
        32
    );
    assert_eq!(
        prk.to_vec(),
        hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
    );
    let mut okm = [0; 42];
    assert_eq!(
        crypto::hkdf_expand(hash::SHA256, &prk, &hex("f0f1f2f3f4f5f6f7f8f9"), &mut okm),
        42
    );
    assert_eq!(
        okm.to_vec(),
        hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    );
}

#[test]
fn quic_initial_keys_match_rfc_9001_appendix_a() {
    let salt = hex("38762cf7f55934b34d179ae6a4c80cadccbb7f0a");
    let mut initial = [0; 32];
    assert_eq!(
        crypto::hkdf_extract(hash::SHA256, &salt, &hex("8394c8f03e515708"), &mut initial),
        32
    );
    let client = expand(&initial, "client in", 32);
    assert_eq!(
        client,
        hex("c00cf151ca5be075ed0ebfb5c80323c42d6b7db67881289af4008f1f6c357aea")
    );
    assert_eq!(
        expand(&client, "quic key", 16),
        hex("1f369613dd76d5467730efcbe3b1a22d")
    );
    assert_eq!(
        expand(&client, "quic iv", 12),
        hex("fa044b2f42a3fd3b46fb255c")
    );
    let hp = expand(&client, "quic hp", 16);
    assert_eq!(hp, hex("9f50449e04a0e810283a1e9933adedd2"));
    let mut mask = [0; 5];
    let sample = hex("d1b1c98dd7689fb8ec11d242b123dc9b");
    assert_eq!(
        crypto::header_mask(suite::AES_128_GCM_SHA256, &hp, &sample, &mut mask),
        5
    );
    assert_eq!(mask.to_vec(), hex("437b9aec36"));
}

#[test]
fn chacha20_header_mask_matches_rfc_9001_appendix_a5() {
    let hp = hex("25a282b9e82f06f21f488917a4fc8f1b73573685608597d0efcb076b0ab7a7a4");
    let mut mask = [0; 5];
    let sample = hex("5e5cd55c41f69080575d7999c25a5bfb");
    assert_eq!(
        crypto::header_mask(suite::CHACHA20_POLY1305_SHA256, &hp, &sample, &mut mask),
        5
    );
    assert_eq!(mask.to_vec(), hex("aefefe7d03"));
}

#[test]
fn sealed_data_opens_and_tampered_data_is_rejected() {
    for (code, key_length) in [
        (suite::AES_128_GCM_SHA256, 16),
        (suite::AES_256_GCM_SHA384, 32),
        (suite::CHACHA20_POLY1305_SHA256, 32),
    ] {
        let key = vec![7; key_length];
        let nonce = [1; 12];
        let mut buffer = b"attack at dawn".to_vec();
        buffer.resize(buffer.len() + 16, 0);
        assert_eq!(
            crypto::aead_seal(code, &key, &nonce, b"header", &mut buffer, 14),
            30
        );
        let mut tampered = buffer.clone();
        assert_eq!(
            crypto::aead_open(code, &key, &nonce, b"header", &mut buffer),
            14
        );
        assert_eq!(&buffer[..14], b"attack at dawn");
        tampered[3] ^= 1;
        assert_eq!(
            crypto::aead_open(code, &key, &nonce, b"header", &mut tampered),
            Failure::Rejected as i64
        );
    }
    let mut small = [0; 4];
    assert_eq!(
        crypto::aead_seal(
            suite::AES_128_GCM_SHA256,
            &[0; 16],
            &[0; 12],
            &[],
            &mut small,
            4
        ),
        Failure::TooSmall as i64
    );
    assert_eq!(
        crypto::aead_seal(0x1304, &[0; 16], &[0; 12], &[], &mut small, 0),
        Failure::Unsupported as i64
    );
}

#[test]
fn two_ephemeral_keys_agree_once() {
    for (code, share) in [(group::X25519, 32), (group::SECP256R1, 65)] {
        let mut first_public = vec![0; share];
        let mut second_public = vec![0; share];
        let first = crypto::agreement_create(code, &mut first_public);
        let second = crypto::agreement_create(code, &mut second_public);
        assert!(first > 0 && second > 0);
        let mut one = [0; 32];
        let mut two = [0; 32];
        assert_eq!(
            crypto::agreement_finish(first, &second_public, &mut one),
            32
        );
        assert_eq!(
            crypto::agreement_finish(second, &first_public, &mut two),
            32
        );
        assert_eq!(one, two);
        assert_eq!(
            crypto::agreement_finish(first, &second_public, &mut one),
            Failure::InvalidHandle as i64
        );
    }
}

#[test]
fn digests_hmacs_and_random_bytes() {
    let mut out = [0; 48];
    assert_eq!(crypto::digest(hash::SHA256, b"abc", &mut out), 32);
    assert_eq!(
        out[..32].to_vec(),
        hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(crypto::digest(hash::SHA384, b"", &mut out), 48);
    assert_eq!(
        crypto::hmac(hash::SHA256, b"key", b"data", &mut out[..8]),
        Failure::TooSmall as i64
    );
    let mut first = [0; 32];
    let mut second = [0; 32];
    assert_eq!(crypto::random(&mut first), 1);
    assert_eq!(crypto::random(&mut second), 1);
    assert_ne!(first, second);
}

const CA: &[u8] = include_bytes!("fixtures/ca.der");
const LEAF: &[u8] = include_bytes!("fixtures/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("fixtures/leaf.key.der");
// 2027-01-01, inside the fixtures' validity.
const NOW: i64 = 1_798_761_600;

fn listed(certificates: &[&[u8]]) -> Vec<u8> {
    let mut list = Vec::new();
    for certificate in certificates {
        let size = u32::try_from(certificate.len()).unwrap().to_be_bytes();
        list.extend_from_slice(&size[1..]);
        list.extend_from_slice(certificate);
    }
    list
}

#[test]
fn a_chain_verifies_only_for_its_anchor_name_and_time() {
    let chain = listed(&[LEAF]);
    let anchor = listed(&[CA]);
    assert_eq!(
        crypto::certificate_verify(&chain, b"localhost", NOW, &anchor),
        1
    );
    assert_eq!(
        crypto::certificate_verify(&chain, b"127.0.0.1", NOW, &anchor),
        1
    );
    assert_eq!(
        crypto::certificate_verify(&chain, b"localhost", NOW, &[]),
        Failure::UnknownIssuer as i64
    );
    assert_eq!(
        crypto::certificate_verify(&chain, b"example.com", NOW, &anchor),
        Failure::WrongName as i64
    );
    assert_eq!(
        crypto::certificate_verify(&chain, b"localhost", 7_258_118_400, &anchor),
        Failure::Expired as i64
    );
    assert_eq!(
        crypto::certificate_verify(&chain[..10], b"localhost", NOW, &anchor),
        Failure::Invalid as i64
    );
}

#[test]
fn a_loaded_key_signs_what_its_certificate_verifies() {
    let key = crypto::key_load(LEAF_KEY);
    assert!(key > 0, "load failed: {key}");
    let mut signature = [0; 128];
    let message = b"TLS 1.3, server CertificateVerify";
    let size = crypto::sign(key, scheme::ECDSA_SECP256R1_SHA256, message, &mut signature);
    assert!(size > 0, "sign failed: {size}");
    let signature = &signature[..usize::try_from(size).unwrap()];
    assert_eq!(
        crypto::verify(scheme::ECDSA_SECP256R1_SHA256, LEAF, message, signature),
        1
    );
    assert_eq!(
        crypto::verify(
            scheme::ECDSA_SECP256R1_SHA256,
            LEAF,
            b"another message",
            signature
        ),
        Failure::Rejected as i64
    );
    assert_eq!(
        crypto::sign(key, scheme::ED25519, message, &mut [0; 64]),
        Failure::Unsupported as i64
    );
    assert_eq!(crypto::release(key), 1);
    assert_eq!(
        crypto::sign(key, scheme::ECDSA_SECP256R1_SHA256, message, &mut [0; 128]),
        Failure::InvalidHandle as i64
    );
}
