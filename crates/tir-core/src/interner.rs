use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StringId(u32);

impl StringId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Interner {
    strings: Vec<Arc<str>>,
    ids: HashMap<Arc<str>, StringId>,
}

impl Interner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, value: impl AsRef<str>) -> Result<StringId, InternerError> {
        let value = value.as_ref();
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }

        let id = StringId(u32::try_from(self.strings.len()).map_err(|_| InternerError)?);
        let value: Arc<str> = Arc::from(value);
        self.ids.insert(Arc::clone(&value), id);
        self.strings.push(value);
        Ok(id)
    }

    #[must_use]
    pub fn resolve(&self, id: StringId) -> Option<&str> {
        self.strings.get(id.index()).map(AsRef::as_ref)
    }

    #[must_use]
    pub fn strings(&self) -> impl ExactSizeIterator<Item = &str> {
        self.strings.iter().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }
}

impl Default for Interner {
    fn default() -> Self {
        let empty: Arc<str> = Arc::from("");
        Self {
            strings: vec![Arc::clone(&empty)],
            ids: HashMap::from([(empty, StringId(0))]),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternerError;

impl fmt::Display for InternerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("string table exceeds the u32 index space")
    }
}

impl std::error::Error for InternerError {}
