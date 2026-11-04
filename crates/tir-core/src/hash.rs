use std::fmt;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    #[must_use]
    pub fn of(bytes: impl AsRef<[u8]>) -> Self {
        let mut hasher = ContentHasher::new();
        hasher.update(bytes.as_ref());
        hasher.finish()
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub struct ContentHasher(blake3::Hasher);

impl ContentHasher {
    #[must_use]
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    #[must_use]
    pub fn finish(self) -> ContentHash {
        ContentHash::from_bytes(*self.0.finalize().as_bytes())
    }
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl AsRef<[u8]> for ContentHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
