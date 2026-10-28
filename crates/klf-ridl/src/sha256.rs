//! SHA-256 for the module-metadata hash (section 24.3) and cache keys.

/// The SHA-256 digest of `data`, with the processor's SHA instructions where
/// it has them.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(data).into()
}

/// Lowercase hex of a digest.
#[must_use]
pub fn hex(digest: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(digest.len() * 2);
    for byte in digest {
        s.push(DIGITS[(byte >> 4) as usize] as char);
        s.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    s
}
