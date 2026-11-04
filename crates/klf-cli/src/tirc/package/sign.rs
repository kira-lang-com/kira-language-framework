use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use klf_host_crypto::CertificateSignatureAlgorithm as HostCertificateSignatureAlgorithm;
use ml_dsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use ml_dsa::{Keypair, MlDsa65, SignatureEncoding, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{Value, json};

pub(super) fn sign_digest(identity_path: &Path, digest: &[u8; 32]) -> Result<Vec<u8>, String> {
    let identity_bytes = fs::read(identity_path).map_err(|error| {
        format!(
            "cannot read signing identity '{}': {error}",
            identity_path.display()
        )
    })?;
    let identity = crate::tirc::jcs::parse_json(&identity_bytes, "signing identity")?;
    let object = identity
        .as_object()
        .ok_or_else(|| "signing identity must be a JSON object".to_owned())?;
    let (chain, ed25519_leaf, mldsa65_leaf) = decode_chain(object)?;
    let ed25519_key = decode_key(object, "ed25519PrivateKey")?;
    let mldsa65_key = decode_key(object, "mldsa65PrivateKey")?;
    let message = package_signing_message(digest);

    let handle = klf_host_crypto::key_load(&ed25519_key);
    if handle <= 0 {
        return Err("signing identity has an invalid Ed25519 PKCS#8 key".to_owned());
    }
    let mut ed25519_signature = [0_u8; 64];
    let signed = klf_host_crypto::sign(
        handle,
        klf_host_crypto::scheme::ED25519,
        &message,
        &mut ed25519_signature,
    );
    let released = klf_host_crypto::release(handle);
    if signed != ed25519_signature.len() as i64 || released != 1 {
        return Err("Ed25519 package signing failed".to_owned());
    }
    if klf_host_crypto::verify(
        klf_host_crypto::scheme::ED25519,
        &ed25519_leaf,
        &message,
        &ed25519_signature,
    ) != 1
    {
        return Err("Ed25519 leaf certificate does not match its private key".to_owned());
    }

    let mldsa_key = SigningKey::<MlDsa65>::from_pkcs8_der(&mldsa65_key)
        .map_err(|_| "signing identity has an invalid ML-DSA-65 PKCS#8 key".to_owned())?;
    let mldsa_leaf_fields = certificate_fields(&mldsa65_leaf)
        .ok_or_else(|| "ML-DSA-65 leaf certificate has invalid DER".to_owned())?;
    let mldsa_leaf_public_key =
        VerifyingKey::<MlDsa65>::from_public_key_der(mldsa_leaf_fields.subject_public_key_info)
            .map_err(|_| "ML-DSA-65 leaf certificate has an invalid public key".to_owned())?;
    if mldsa_key.verifying_key() != mldsa_leaf_public_key {
        return Err("ML-DSA-65 leaf certificate does not match its private key".to_owned());
    }
    let mldsa_signature = mldsa_key.sign(&message);
    mldsa_leaf_public_key
        .verify(&message, &mldsa_signature)
        .map_err(|_| "ML-DSA-65 package signature verification failed".to_owned())?;
    let signature_record = json!({
        "chain": chain,
        "ed25519": STANDARD.encode(ed25519_signature),
        "mldsa65": STANDARD.encode(mldsa_signature.to_bytes().as_slice()),
    });
    Ok(crate::tirc::jcs::canonical_json(&signature_record)?.into_bytes())
}

pub(in crate::tirc) fn verify_package_signature(
    signature_bytes: &[u8],
    digest: &[u8; 32],
) -> Result<(), String> {
    let signature_record = crate::tirc::jcs::parse_json(signature_bytes, "developer signature")?;
    let object = signature_record
        .as_object()
        .ok_or_else(|| "developer signature must be a JSON object".to_owned())?;
    if object.len() != 3
        || !object.contains_key("chain")
        || !object.contains_key("ed25519")
        || !object.contains_key("mldsa65")
    {
        return Err("developer signature must contain only chain, ed25519 and mldsa65".to_owned());
    }
    let (_, ed25519_leaf, mldsa65_leaf) = decode_chain(object)?;
    let ed25519_signature = object
        .get("ed25519")
        .and_then(Value::as_str)
        .ok_or_else(|| "developer signature `ed25519` must be a string".to_owned())?;
    let ed25519_signature = decode_base64(ed25519_signature, "developer signature `ed25519`")?;
    let mldsa65_signature = object
        .get("mldsa65")
        .and_then(Value::as_str)
        .ok_or_else(|| "developer signature `mldsa65` must be a string".to_owned())?;
    let mldsa65_signature = decode_base64(mldsa65_signature, "developer signature `mldsa65`")?;
    let message = package_signing_message(digest);

    if klf_host_crypto::verify(
        klf_host_crypto::scheme::ED25519,
        &ed25519_leaf,
        &message,
        &ed25519_signature,
    ) != 1
    {
        return Err("developer package Ed25519 signature is invalid".to_owned());
    }

    let mldsa_leaf_fields = certificate_fields(&mldsa65_leaf)
        .ok_or_else(|| "ML-DSA-65 leaf certificate has invalid DER".to_owned())?;
    let mldsa_leaf_public_key =
        VerifyingKey::<MlDsa65>::from_public_key_der(mldsa_leaf_fields.subject_public_key_info)
            .map_err(|_| "ML-DSA-65 leaf certificate has an invalid public key".to_owned())?;
    let mldsa65_signature = ml_dsa::Signature::<MlDsa65>::try_from(mldsa65_signature.as_slice())
        .map_err(|_| "developer package ML-DSA-65 signature has invalid encoding".to_owned())?;
    mldsa_leaf_public_key
        .verify(&message, &mldsa65_signature)
        .map_err(|_| "developer package ML-DSA-65 signature is invalid".to_owned())?;

    Ok(())
}

fn decode_key(object: &serde_json::Map<String, Value>, field: &str) -> Result<Vec<u8>, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("signing identity is missing base64 field `{field}`"))?;
    decode_base64(value, &format!("signing identity field `{field}`"))
}

fn decode_chain(
    object: &serde_json::Map<String, Value>,
) -> Result<(Value, Vec<u8>, Vec<u8>), String> {
    let chain = object
        .get("chain")
        .and_then(Value::as_object)
        .ok_or_else(|| "signing identity `chain` must be an object".to_owned())?;
    if chain.len() != 2 || !chain.contains_key("ed25519") || !chain.contains_key("mldsa65") {
        return Err(
            "signing identity `chain` must contain only `ed25519` and `mldsa65`".to_owned(),
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock predates the Unix epoch".to_owned())?
        .as_secs();
    let now =
        i64::try_from(now).map_err(|_| "system clock is outside the supported range".to_owned())?;
    let mut leaves = [None, None];
    let mut leaf_subjects = [None, None];
    let mut root_certificates = [None, None];
    let mut certificate_chains: [Option<Vec<Vec<u8>>>; 2] = [None, None];
    for (algorithm_index, algorithm) in ["ed25519", "mldsa65"].into_iter().enumerate() {
        let certificates = chain
            .get(algorithm)
            .and_then(Value::as_array)
            .filter(|certificates| !certificates.is_empty())
            .ok_or_else(|| {
                format!("signing identity `chain.{algorithm}` must be a nonempty array")
            })?;
        let mut expected_subject: Option<Vec<u8>> = None;
        let mut final_subject: Option<Vec<u8>> = None;
        let mut chain_der = Vec::with_capacity(certificates.len());
        for (index, certificate) in certificates.iter().enumerate() {
            let encoded = certificate.as_str().ok_or_else(|| {
                format!("signing identity `chain.{algorithm}[{index}]` must be a string")
            })?;
            let der = decode_base64(
                encoded,
                &format!("signing identity `chain.{algorithm}[{index}]`"),
            )?;
            let fields = certificate_fields(&der).ok_or_else(|| {
                format!(
                    "signing identity `chain.{algorithm}[{index}]` is not a valid DER certificate"
                )
            })?;
            if now < fields.validity.0 || now > fields.validity.1 {
                return Err(format!(
                    "signing identity `chain.{algorithm}[{index}]` is not currently valid"
                ));
            }
            if index == 0 && fields.validity.1 - fields.validity.0 > 182 * 24 * 60 * 60 {
                return Err(format!(
                    "signing identity `chain.{algorithm}[{index}]` validity exceeds 182 days"
                ));
            }
            let issuer = fields.issuer.to_vec();
            let subject = fields.subject.to_vec();
            if expected_subject
                .as_ref()
                .is_some_and(|expected| expected.as_slice() != subject.as_slice())
            {
                return Err(format!(
                    "signing identity `chain.{algorithm}[{index}]` issuer does not match the prior certificate"
                ));
            }
            if index + 1 == certificates.len() {
                root_certificates[algorithm_index] = Some(der.clone());
            }
            if index == 0 {
                leaf_subjects[algorithm_index] = Some(subject.clone());
                leaves[algorithm_index] = Some(der.clone());
            }
            expected_subject = Some(issuer);
            final_subject = Some(subject);
            chain_der.push(der);
        }
        if expected_subject != final_subject {
            return Err(format!(
                "signing identity `chain.{algorithm}` root is not self-issued"
            ));
        }
        certificate_chains[algorithm_index] = Some(chain_der);
    }
    if leaf_subjects[0] != leaf_subjects[1] {
        return Err(
            "Ed25519 and ML-DSA-65 leaf certificates do not identify the same developer".to_owned(),
        );
    }
    if root_certificates[0] != root_certificates[1] {
        return Err("signing identity chains do not share the same root certificate".to_owned());
    }
    for (algorithm_index, algorithm) in ["ed25519", "mldsa65"].into_iter().enumerate() {
        let chain_der = certificate_chains[algorithm_index]
            .as_ref()
            .ok_or_else(|| format!("signing identity `chain.{algorithm}` has no certificates"))?;
        for (index, certificate_der) in chain_der.iter().enumerate() {
            let certificate = certificate_fields(certificate_der).ok_or_else(|| {
                format!(
                    "signing identity `chain.{algorithm}[{index}]` is not a valid DER certificate"
                )
            })?;
            let issuer_index = (index + 1).min(chain_der.len() - 1);
            let issuer_der = &chain_der[issuer_index];
            let issuer = certificate_fields(issuer_der).ok_or_else(|| {
                format!(
                    "signing identity `chain.{algorithm}[{issuer_index}]` is not a valid DER certificate"
                )
            })?;
            if !verify_certificate_signature(&certificate, issuer_der, &issuer) {
                let relation = if index + 1 == chain_der.len() {
                    "root self-signature"
                } else {
                    "issuer signature"
                };
                return Err(format!(
                    "signing identity `chain.{algorithm}[{index}]` has an invalid {relation}"
                ));
            }
        }
    }
    let ed25519_leaf = leaves[0]
        .take()
        .ok_or_else(|| "signing identity `chain.ed25519` has no leaf certificate".to_owned())?;
    let mldsa65_leaf = leaves[1]
        .take()
        .ok_or_else(|| "signing identity `chain.mldsa65` has no leaf certificate".to_owned())?;
    Ok((Value::Object(chain.clone()), ed25519_leaf, mldsa65_leaf))
}

fn decode_base64(value: &str, label: &str) -> Result<Vec<u8>, String> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| format!("{label} is not valid standard base64"))?;
    if STANDARD.encode(&decoded) != value {
        return Err(format!("{label} is not canonical standard base64"));
    }
    Ok(decoded)
}

struct CertificateFields<'a> {
    issuer: &'a [u8],
    subject: &'a [u8],
    subject_public_key_info: &'a [u8],
    validity: (i64, i64),
    tbs_certificate: &'a [u8],
    signature_algorithm_oid: &'a [u8],
    signature_algorithm_parameters: Option<&'a [u8]>,
    signature: &'a [u8],
}

fn certificate_fields(bytes: &[u8]) -> Option<CertificateFields<'_>> {
    let mut certificate_cursor = 0;
    let certificate = der_element(bytes, &mut certificate_cursor, bytes.len())?;
    if certificate.tag != 0x30
        || certificate.end != bytes.len()
        || certificate_cursor != bytes.len()
    {
        return None;
    }

    let mut certificate_content = certificate.content_start;
    let tbs = der_element(bytes, &mut certificate_content, certificate.end)?;
    let signature_algorithm = der_element(bytes, &mut certificate_content, certificate.end)?;
    let signature = der_element(bytes, &mut certificate_content, certificate.end)?;
    if tbs.tag != 0x30
        || signature_algorithm.tag != 0x30
        || signature.tag != 0x03
        || certificate_content != certificate.end
    {
        return None;
    }

    let mut cursor = tbs.content_start;
    if bytes.get(cursor) == Some(&0xa0) {
        let version = der_element(bytes, &mut cursor, tbs.end)?;
        let mut version_cursor = version.content_start;
        let encoded_version = der_element(bytes, &mut version_cursor, version.end)?;
        if version.tag != 0xa0 || encoded_version.tag != 0x02 || version_cursor != version.end {
            return None;
        }
    }
    if der_element(bytes, &mut cursor, tbs.end)?.tag != 0x02 {
        return None;
    }
    let tbs_signature_algorithm = der_element(bytes, &mut cursor, tbs.end)?;
    if tbs_signature_algorithm.tag != 0x30 {
        return None;
    }
    let issuer = der_element(bytes, &mut cursor, tbs.end)?;
    let validity = der_element(bytes, &mut cursor, tbs.end)?;
    let subject = der_element(bytes, &mut cursor, tbs.end)?;
    let spki = der_element(bytes, &mut cursor, tbs.end)?;
    if issuer.tag != 0x30 || validity.tag != 0x30 || subject.tag != 0x30 || spki.tag != 0x30 {
        return None;
    }
    if bytes[tbs_signature_algorithm.start..tbs_signature_algorithm.end]
        != bytes[signature_algorithm.start..signature_algorithm.end]
    {
        return None;
    }
    let (signature_algorithm_oid, signature_algorithm_parameters) =
        algorithm_identifier(bytes, signature_algorithm)?;
    if bytes.get(signature.content_start) != Some(&0) || signature.end - signature.content_start < 2
    {
        return None;
    }
    let validity = certificate_validity(bytes, validity)?;

    let mut previous_optional_tag = 0_u8;
    while cursor < tbs.end {
        let optional = der_element(bytes, &mut cursor, tbs.end)?;
        if !matches!(optional.tag, 0x81 | 0x82 | 0xa3) || optional.tag <= previous_optional_tag {
            return None;
        }
        previous_optional_tag = optional.tag;
    }

    let mut spki_cursor = spki.content_start;
    let algorithm = der_element(bytes, &mut spki_cursor, spki.end)?;
    let public_key = der_element(bytes, &mut spki_cursor, spki.end)?;
    if algorithm.tag != 0x30
        || public_key.tag != 0x03
        || spki_cursor != spki.end
        || bytes.get(public_key.content_start) != Some(&0)
        || public_key.end == public_key.content_start + 1
    {
        return None;
    }

    Some(CertificateFields {
        issuer: &bytes[issuer.start..issuer.end],
        subject: &bytes[subject.start..subject.end],
        subject_public_key_info: &bytes[spki.start..spki.end],
        validity,
        tbs_certificate: &bytes[tbs.start..tbs.end],
        signature_algorithm_oid,
        signature_algorithm_parameters,
        signature: &bytes[signature.content_start + 1..signature.end],
    })
}

fn algorithm_identifier<'a>(
    bytes: &'a [u8],
    algorithm: DerElement,
) -> Option<(&'a [u8], Option<&'a [u8]>)> {
    if algorithm.tag != 0x30 {
        return None;
    }
    let mut cursor = algorithm.content_start;
    let oid = der_element(bytes, &mut cursor, algorithm.end)?;
    if oid.tag != 0x06 || oid.content_start == oid.end {
        return None;
    }
    let parameters = if cursor < algorithm.end {
        let parameters = der_element(bytes, &mut cursor, algorithm.end)?;
        Some(&bytes[parameters.start..parameters.end])
    } else {
        None
    };
    (cursor == algorithm.end).then_some((&bytes[oid.content_start..oid.end], parameters))
}

fn algorithm_identifier_der(der: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let mut cursor = 0;
    let sequence = der_element(der, &mut cursor, der.len())?;
    if sequence.tag != 0x30 || cursor != der.len() {
        return None;
    }
    algorithm_identifier(der, sequence)
}

#[derive(Clone, Copy)]
enum TirCertificateSignatureAlgorithm {
    Ed25519,
    MlDsa65,
    EcdsaSha256,
    EcdsaSha384,
    RsaPkcs1Sha256,
    RsaPkcs1Sha384,
    RsaPkcs1Sha512,
    RsaPssSha256,
    RsaPssSha384,
    RsaPssSha512,
}

fn certificate_signature_algorithm(
    certificate: &CertificateFields<'_>,
) -> Option<TirCertificateSignatureAlgorithm> {
    const ED25519: &[u8] = &[0x2b, 0x65, 0x70];
    const ML_DSA_65: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x12];
    const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
    const ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
    const RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
    const RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
    const RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
    const RSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];

    let parameters = certificate.signature_algorithm_parameters;
    let no_parameters = parameters.is_none();
    let rsa_parameters = parameters.is_none_or(|value| value == [0x05, 0x00]);
    match certificate.signature_algorithm_oid {
        ED25519 if no_parameters => Some(TirCertificateSignatureAlgorithm::Ed25519),
        ML_DSA_65 if no_parameters => Some(TirCertificateSignatureAlgorithm::MlDsa65),
        ECDSA_SHA256 if no_parameters => Some(TirCertificateSignatureAlgorithm::EcdsaSha256),
        ECDSA_SHA384 if no_parameters => Some(TirCertificateSignatureAlgorithm::EcdsaSha384),
        RSA_SHA256 if rsa_parameters => Some(TirCertificateSignatureAlgorithm::RsaPkcs1Sha256),
        RSA_SHA384 if rsa_parameters => Some(TirCertificateSignatureAlgorithm::RsaPkcs1Sha384),
        RSA_SHA512 if rsa_parameters => Some(TirCertificateSignatureAlgorithm::RsaPkcs1Sha512),
        RSA_PSS => match pss_parameters_hash(parameters?)? {
            PssHash::Sha256 => Some(TirCertificateSignatureAlgorithm::RsaPssSha256),
            PssHash::Sha384 => Some(TirCertificateSignatureAlgorithm::RsaPssSha384),
            PssHash::Sha512 => Some(TirCertificateSignatureAlgorithm::RsaPssSha512),
        },
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PssHash {
    Sha256,
    Sha384,
    Sha512,
}

fn pss_parameters_hash(parameters: &[u8]) -> Option<PssHash> {
    const MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
    let mut cursor = 0;
    let sequence = der_element(parameters, &mut cursor, parameters.len())?;
    if sequence.tag != 0x30 || cursor != parameters.len() {
        return None;
    }

    let mut cursor = sequence.content_start;
    let mut previous_tag = 0_u8;
    let mut hash = None;
    let mut mgf_hash = None;
    let mut salt_length = None;
    let mut trailer_field = 1;
    while cursor < sequence.end {
        let field = der_element(parameters, &mut cursor, sequence.end)?;
        if field.tag <= previous_tag || !matches!(field.tag, 0xa0 | 0xa1 | 0xa2 | 0xa3) {
            return None;
        }
        previous_tag = field.tag;
        match field.tag {
            0xa0 => {
                let algorithm = explicit_algorithm_identifier(parameters, field)?;
                hash = Some(hash_algorithm_identifier(algorithm.0, algorithm.1)?);
            }
            0xa1 => {
                let (oid, parameters) = explicit_algorithm_identifier(parameters, field)?;
                if oid != MGF1 {
                    return None;
                }
                let (hash_oid, hash_parameters) = algorithm_identifier_der(parameters?)?;
                mgf_hash = Some(hash_algorithm_identifier(hash_oid, hash_parameters)?);
            }
            0xa2 => {
                let integer = explicit_integer(parameters, field)?;
                salt_length = Some(der_nonnegative_integer(parameters, integer)?);
            }
            0xa3 => {
                let integer = explicit_integer(parameters, field)?;
                trailer_field = der_nonnegative_integer(parameters, integer)?;
            }
            _ => return None,
        }
    }

    let hash = hash?;
    if mgf_hash? != hash || trailer_field != 1 {
        return None;
    }
    let expected_salt_length = match hash {
        PssHash::Sha256 => 32,
        PssHash::Sha384 => 48,
        PssHash::Sha512 => 64,
    };
    (salt_length? == expected_salt_length).then_some(hash)
}

fn explicit_algorithm_identifier<'a>(
    bytes: &'a [u8],
    explicit: DerElement,
) -> Option<(&'a [u8], Option<&'a [u8]>)> {
    let mut cursor = explicit.content_start;
    let sequence = der_element(bytes, &mut cursor, explicit.end)?;
    if explicit.tag & 0xe0 != 0xa0 || cursor != explicit.end {
        return None;
    }
    algorithm_identifier(bytes, sequence)
}

fn explicit_integer(bytes: &[u8], explicit: DerElement) -> Option<DerElement> {
    let mut cursor = explicit.content_start;
    let integer = der_element(bytes, &mut cursor, explicit.end)?;
    if explicit.tag & 0xe0 != 0xa0 || integer.tag != 0x02 || cursor != explicit.end {
        return None;
    }
    Some(integer)
}

fn hash_algorithm_identifier(oid: &[u8], parameters: Option<&[u8]>) -> Option<PssHash> {
    const SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    const SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
    const SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
    if parameters.is_some_and(|parameters| parameters != [0x05, 0x00]) {
        return None;
    }
    match oid {
        SHA256 => Some(PssHash::Sha256),
        SHA384 => Some(PssHash::Sha384),
        SHA512 => Some(PssHash::Sha512),
        _ => None,
    }
}

fn der_nonnegative_integer(bytes: &[u8], integer: DerElement) -> Option<u32> {
    if integer.tag != 0x02 || integer.content_start == integer.end {
        return None;
    }
    let encoded = bytes.get(integer.content_start..integer.end)?;
    if encoded[0] & 0x80 != 0 || (encoded.len() > 1 && encoded[0] == 0 && encoded[1] & 0x80 == 0) {
        return None;
    }
    encoded.iter().try_fold(0_u32, |value, byte| {
        value.checked_mul(256)?.checked_add(u32::from(*byte))
    })
}

fn verify_certificate_signature(
    certificate: &CertificateFields<'_>,
    issuer_der: &[u8],
    issuer: &CertificateFields<'_>,
) -> bool {
    let Some(algorithm) = certificate_signature_algorithm(certificate) else {
        return false;
    };
    match algorithm {
        TirCertificateSignatureAlgorithm::Ed25519 => {
            klf_host_crypto::verify(
                klf_host_crypto::scheme::ED25519,
                issuer_der,
                certificate.tbs_certificate,
                certificate.signature,
            ) == 1
        }
        TirCertificateSignatureAlgorithm::MlDsa65 => {
            let Ok(public_key) =
                VerifyingKey::<MlDsa65>::from_public_key_der(issuer.subject_public_key_info)
            else {
                return false;
            };
            let Ok(signature) = ml_dsa::Signature::<MlDsa65>::try_from(certificate.signature)
            else {
                return false;
            };
            public_key
                .verify(certificate.tbs_certificate, &signature)
                .is_ok()
        }
        algorithm => {
            let host_algorithm = match algorithm {
                TirCertificateSignatureAlgorithm::EcdsaSha256 => {
                    HostCertificateSignatureAlgorithm::EcdsaSha256
                }
                TirCertificateSignatureAlgorithm::EcdsaSha384 => {
                    HostCertificateSignatureAlgorithm::EcdsaSha384
                }
                TirCertificateSignatureAlgorithm::RsaPkcs1Sha256 => {
                    HostCertificateSignatureAlgorithm::RsaPkcs1Sha256
                }
                TirCertificateSignatureAlgorithm::RsaPkcs1Sha384 => {
                    HostCertificateSignatureAlgorithm::RsaPkcs1Sha384
                }
                TirCertificateSignatureAlgorithm::RsaPkcs1Sha512 => {
                    HostCertificateSignatureAlgorithm::RsaPkcs1Sha512
                }
                TirCertificateSignatureAlgorithm::RsaPssSha256 => {
                    HostCertificateSignatureAlgorithm::RsaPssSha256
                }
                TirCertificateSignatureAlgorithm::RsaPssSha384 => {
                    HostCertificateSignatureAlgorithm::RsaPssSha384
                }
                TirCertificateSignatureAlgorithm::RsaPssSha512 => {
                    HostCertificateSignatureAlgorithm::RsaPssSha512
                }
                TirCertificateSignatureAlgorithm::Ed25519
                | TirCertificateSignatureAlgorithm::MlDsa65 => return false,
            };
            klf_host_crypto::certificate_signature_verify(
                host_algorithm,
                issuer_der,
                certificate.tbs_certificate,
                certificate.signature,
            ) == 1
        }
    }
}

fn certificate_validity(bytes: &[u8], validity: DerElement) -> Option<(i64, i64)> {
    let mut cursor = validity.content_start;
    let not_before = der_element(bytes, &mut cursor, validity.end)?;
    let not_after = der_element(bytes, &mut cursor, validity.end)?;
    if validity.tag != 0x30 || cursor != validity.end {
        return None;
    }
    let not_before = parse_certificate_time(
        not_before.tag,
        bytes.get(not_before.content_start..not_before.end)?,
    )?;
    let not_after = parse_certificate_time(
        not_after.tag,
        bytes.get(not_after.content_start..not_after.end)?,
    )?;
    (not_before < not_after).then_some((not_before, not_after))
}

fn parse_certificate_time(tag: u8, encoded: &[u8]) -> Option<i64> {
    if encoded.last() != Some(&b'Z') {
        return None;
    }
    let (year, offset) = match tag {
        0x17 if encoded.len() == 13 => {
            let year = decimal(&encoded[..2])?;
            (if year >= 50 { 1900 + year } else { 2000 + year }, 2)
        }
        0x18 if encoded.len() == 15 => (decimal(&encoded[..4])?, 4),
        _ => return None,
    };
    let month = decimal(&encoded[offset..offset + 2])?;
    let day = decimal(&encoded[offset + 2..offset + 4])?;
    let hour = decimal(&encoded[offset + 4..offset + 6])?;
    let minute = decimal(&encoded[offset + 6..offset + 8])?;
    let second = decimal(&encoded[offset + 8..offset + 10])?;
    if year == 0
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let year = i64::from(year);
    let month = i64::from(month);
    let day = i64::from(day);
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_epoch = era * 146_097 + day_of_era - 719_468;
    Some(
        days_since_epoch * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second),
    )
}

fn decimal(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0_u32, |value, digit| {
        if !digit.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(u32::from(*digit - b'0'))
    })
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

struct DerElement {
    tag: u8,
    start: usize,
    content_start: usize,
    end: usize,
}

fn der_element(bytes: &[u8], cursor: &mut usize, limit: usize) -> Option<DerElement> {
    let start = *cursor;
    if start >= limit || limit > bytes.len() {
        return None;
    }
    let tag = *bytes.get(*cursor)?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    *cursor = (*cursor).checked_add(1)?;
    if *cursor >= limit {
        return None;
    }
    let first_length = *bytes.get(*cursor)?;
    *cursor = (*cursor).checked_add(1)?;
    let length = if first_length & 0x80 == 0 {
        usize::from(first_length)
    } else {
        let length_bytes = usize::from(first_length & 0x7f);
        if length_bytes == 0 || length_bytes > std::mem::size_of::<usize>() {
            return None;
        }
        let length_end = (*cursor).checked_add(length_bytes)?;
        if length_end > limit || bytes.get(*cursor) == Some(&0) {
            return None;
        }
        let mut length = 0_usize;
        while *cursor < length_end {
            length = length
                .checked_mul(256)?
                .checked_add(usize::from(*bytes.get(*cursor)?))?;
            *cursor += 1;
        }
        if length < 128 {
            return None;
        }
        length
    };
    let content_start = *cursor;
    let end = content_start.checked_add(length)?;
    if end > limit {
        return None;
    }
    *cursor = end;
    Some(DerElement {
        tag,
        start,
        content_start,
        end,
    })
}

fn package_signing_message(digest: &[u8; 32]) -> Vec<u8> {
    let mut message = b"TIR package v1\0".to_vec();
    message.extend_from_slice(digest);
    message
}
