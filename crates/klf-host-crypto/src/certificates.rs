//! Certificate path validation and signatures made by a certificate's key.
//!
//! A certificate list crosses as TLS writes it in a Certificate message
//! (RFC 8446 section 4.4.2): each DER certificate behind a 24-bit big-endian
//! length, end entity first.

use rustls_pki_types::{CertificateDer, ServerName, SignatureVerificationAlgorithm, UnixTime};
use webpki::{EndEntityCert, Error, KeyUsage, ring as algorithms};

use crate::{Failure, scheme, word};

/// What a chain's own signatures may use. Certificates are still signed with
/// PKCS#1 v1.5, which TLS 1.3 forbids only in handshake signatures (RFC 8446
/// section 4.2.3).
static CHAIN_ALGORITHMS: &[&dyn SignatureVerificationAlgorithm] = &[
    algorithms::ECDSA_P256_SHA256,
    algorithms::ECDSA_P256_SHA384,
    algorithms::ECDSA_P384_SHA256,
    algorithms::ECDSA_P384_SHA384,
    algorithms::RSA_PSS_2048_8192_SHA256_LEGACY_KEY,
    algorithms::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
    algorithms::RSA_PSS_2048_8192_SHA512_LEGACY_KEY,
    algorithms::RSA_PKCS1_2048_8192_SHA256,
    algorithms::RSA_PKCS1_2048_8192_SHA384,
    algorithms::RSA_PKCS1_2048_8192_SHA512,
    algorithms::ED25519,
];

fn handshake_algorithm(code: i64) -> Result<&'static dyn SignatureVerificationAlgorithm, Failure> {
    match code {
        scheme::ECDSA_SECP256R1_SHA256 => Ok(algorithms::ECDSA_P256_SHA256),
        scheme::ECDSA_SECP384R1_SHA384 => Ok(algorithms::ECDSA_P384_SHA384),
        scheme::RSA_PSS_RSAE_SHA256 => Ok(algorithms::RSA_PSS_2048_8192_SHA256_LEGACY_KEY),
        scheme::RSA_PSS_RSAE_SHA384 => Ok(algorithms::RSA_PSS_2048_8192_SHA384_LEGACY_KEY),
        scheme::ED25519 => Ok(algorithms::ED25519),
        _ => Err(Failure::Unsupported),
    }
}

fn failure(error: &Error) -> Failure {
    match error {
        Error::UnknownIssuer => Failure::UnknownIssuer,
        Error::CertExpired { .. } | Error::CertNotValidYet { .. } => Failure::Expired,
        Error::CertNotValidForName(_) => Failure::WrongName,
        Error::InvalidSignatureForPublicKey => Failure::Rejected,
        Error::UnsupportedSignatureAlgorithmContext(_)
        | Error::UnsupportedSignatureAlgorithmForPublicKeyContext(_) => Failure::Unsupported,
        _ => Failure::BadCertificate,
    }
}

/// The certificates of a 24-bit length-prefixed list.
fn certificates(list: &[u8]) -> Result<Vec<CertificateDer<'_>>, Failure> {
    let mut found = Vec::new();
    let mut rest = list;
    while !rest.is_empty() {
        let [high, middle, low, tail @ ..] = rest else {
            return Err(Failure::Invalid);
        };
        let size = usize::from(*high) << 16 | usize::from(*middle) << 8 | usize::from(*low);
        if size == 0 || size > tail.len() {
            return Err(Failure::Invalid);
        }
        found.push(CertificateDer::from(&tail[..size]));
        rest = &tail[size..];
    }
    Ok(found)
}

/// Checks `signature` over `message`, made under `scheme` by the key of the
/// DER certificate `certificate`; answers 1 when it verifies.
#[must_use]
pub fn verify(code: i64, certificate: &[u8], message: &[u8], signature: &[u8]) -> i64 {
    word((|| {
        let algorithm = handshake_algorithm(code)?;
        let der = CertificateDer::from(certificate);
        let entity = EndEntityCert::try_from(&der).map_err(|_| Failure::BadCertificate)?;
        entity
            .verify_signature(algorithm, message, signature)
            .map_err(|error| failure(&error))?;
        Ok(1)
    })())
}

/// Verifies a server's certificate list for `name`, a DNS name or an IP
/// address, at `seconds` since the Unix epoch: the end entity must chain,
/// through the list's other certificates, to a bundled Mozilla root or to one
/// of `anchors` (the same list form, empty for none), and must name the
/// server. Answers 1 when it does.
#[must_use]
pub fn certificate_verify(chain: &[u8], name: &[u8], seconds: i64, anchors: &[u8]) -> i64 {
    word((|| {
        let name = std::str::from_utf8(name).map_err(|_| Failure::WrongName)?;
        let name = ServerName::try_from(name).map_err(|_| Failure::WrongName)?;
        let seconds = u64::try_from(seconds).map_err(|_| Failure::Expired)?;
        let listed = certificates(chain)?;
        let (entity, intermediates) = listed.split_first().ok_or(Failure::BadCertificate)?;
        let extra = certificates(anchors)?;
        let mut trusted = webpki_roots::TLS_SERVER_ROOTS.to_vec();
        for anchor in &extra {
            trusted.push(webpki::anchor_from_trusted_cert(anchor).map_err(|_| Failure::Invalid)?);
        }
        let entity = EndEntityCert::try_from(entity).map_err(|_| Failure::BadCertificate)?;
        entity
            .verify_for_usage(
                CHAIN_ALGORITHMS,
                &trusted,
                intermediates,
                UnixTime::since_unix_epoch(std::time::Duration::from_secs(seconds)),
                KeyUsage::server_auth(),
                None,
                None,
            )
            .map_err(|error| failure(&error))?;
        entity
            .verify_is_valid_for_subject_name(&name)
            .map_err(|error| failure(&error))?;
        Ok(1)
    })())
}
