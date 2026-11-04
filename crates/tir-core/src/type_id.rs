#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeId(u64);

impl TypeId {
    #[must_use]
    pub fn from_canonical_text(text: &str) -> Self {
        let hash = blake3::hash(text.as_bytes());
        let mut prefix = [0; 8];
        prefix.copy_from_slice(&hash.as_bytes()[..8]);
        Self(u64::from_le_bytes(prefix))
    }

    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}
