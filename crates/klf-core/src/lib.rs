//! Shared identifiers and interned names for the KLF kernel.

use std::fmt;
use std::sync::Arc;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Serialize,
            serde::Deserialize,
        )]
        pub struct $name(u32);

        impl $name {
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
    };
}

define_id!(FileId);
define_id!(TypeId);
define_id!(FunctionId);
define_id!(BlockId);
define_id!(ValueId);

/// An immutable UTF-8 name shared across kernel data structures.
#[derive(Clone, Eq, Ord, PartialOrd)]
pub struct Name(Arc<str>);

// Hashed as its text, so a name and a `str` it borrows as hash alike.
impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

// Clones of one name share their text, so most comparisons end at the pointer.
impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Name {
    #[must_use]
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(Arc::from(value.as_ref()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// A name is written as its text, or, inside [`with_name_table`], as its index in
// the table, so a large structure carries each distinct name once.
std::thread_local! {
    static NAME_TABLE: std::cell::RefCell<Option<NameTable>> = const { std::cell::RefCell::new(None) };
}

enum NameTable {
    Writing(std::collections::HashMap<Name, u32>, Vec<Name>),
    Reading(Vec<Name>),
}

/// Runs `write` with names serialized as indices into a table, and returns its
/// result with the table: the names in first-written order.
pub fn writing_name_table<R>(write: impl FnOnce() -> R) -> (R, Vec<Name>) {
    NAME_TABLE.with(|table| {
        *table.borrow_mut() = Some(NameTable::Writing(
            std::collections::HashMap::new(),
            Vec::new(),
        ));
    });
    let result = write();
    let names = NAME_TABLE.with(|table| match table.borrow_mut().take() {
        Some(NameTable::Writing(_, names)) => names,
        _ => Vec::new(),
    });
    (result, names)
}

/// Runs `read` with names deserialized as indices into `names`, the table
/// [`writing_name_table`] returned.
pub fn reading_name_table<R>(names: Vec<Name>, read: impl FnOnce() -> R) -> R {
    NAME_TABLE.with(|table| *table.borrow_mut() = Some(NameTable::Reading(names)));
    let result = read();
    NAME_TABLE.with(|table| *table.borrow_mut() = None);
    result
}

impl serde::Serialize for Name {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let index = NAME_TABLE.with(|table| match &mut *table.borrow_mut() {
            Some(NameTable::Writing(indices, names)) => {
                Some(*indices.entry(self.clone()).or_insert_with(|| {
                    names.push(self.clone());
                    u32::try_from(names.len() - 1).unwrap_or(u32::MAX)
                }))
            }
            _ => None,
        });
        match index {
            Some(index) => serializer.serialize_u32(index),
            None => serializer.serialize_str(&self.0),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Name {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text;
        impl serde::de::Visitor<'_> for Text {
            type Value = Name;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a name")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Name, E> {
                Ok(Name::new(value))
            }
        }
        let reading =
            NAME_TABLE.with(|table| matches!(&*table.borrow(), Some(NameTable::Reading(_))));
        if !reading {
            return deserializer.deserialize_str(Text);
        }
        let index = <u32 as serde::Deserialize>::deserialize(deserializer)?;
        NAME_TABLE
            .with(|table| match &*table.borrow() {
                Some(NameTable::Reading(names)) => names.get(index as usize).cloned(),
                _ => None,
            })
            .ok_or_else(|| serde::de::Error::custom("name index outside the table"))
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A name hashes and compares as its text, so a map keyed by names answers a
/// lookup by `&str` without building one.
impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self(Arc::from(value))
    }
}
