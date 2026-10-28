//! Arenas and handles (`KLF-KSL.md` section 7.2).

use crate::Span;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;

/// A dense index into one arena.
pub struct Handle<T> {
    index: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self {
            index,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self.index as usize
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.index
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<T> Eq for Handle<T> {}

impl<T> PartialOrd for Handle<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Handle<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.index.cmp(&other.index)
    }
}

impl<T> Hash for Handle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.index)
    }
}

/// A half-open range of handles, as an `Emit` covers.
pub struct Range<T> {
    pub start: u32,
    pub end: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> Range<T> {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    pub fn iter(&self) -> impl Iterator<Item = Handle<T>> + use<T> {
        (self.start..self.end).map(Handle::new)
    }

    #[must_use]
    pub fn contains(&self, handle: Handle<T>) -> bool {
        (self.start..self.end).contains(&handle.raw())
    }
}

impl<T> Clone for Range<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Range<T> {}

impl<T> PartialEq for Range<T> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end
    }
}

impl<T> Eq for Range<T> {}

impl<T> fmt::Debug for Range<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}..#{}", self.start, self.end)
    }
}

/// Entries with one span each, addressed by handle in insertion order.
#[derive(Clone, Debug, PartialEq)]
pub struct Arena<T> {
    items: Vec<T>,
    spans: Vec<Span>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            spans: Vec::new(),
        }
    }
}

impl<T> Arena<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// # Panics
    /// Panics past `u32::MAX` entries.
    pub fn append(&mut self, item: T, span: Span) -> Handle<T> {
        let index = u32::try_from(self.items.len()).expect("arena overflow");
        self.items.push(item);
        self.spans.push(span);
        Handle::new(index)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        self.items.get(handle.index())
    }

    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        self.items.get_mut(handle.index())
    }

    #[must_use]
    pub fn span(&self, handle: Handle<T>) -> Span {
        self.spans.get(handle.index()).copied().unwrap_or_default()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (Handle::new(index as u32), item))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Handle<T>, &mut T)> {
        self.items
            .iter_mut()
            .enumerate()
            .map(|(index, item)| (Handle::new(index as u32), item))
    }

    #[must_use]
    pub fn next_handle(&self) -> Handle<T> {
        Handle::new(self.items.len() as u32)
    }

    /// Keeps the entries `keep` accepts, in order, and returns the new handle
    /// of each old one.
    pub fn retain_mapped(
        &mut self,
        mut keep: impl FnMut(Handle<T>) -> bool,
    ) -> Vec<Option<Handle<T>>> {
        let mut map = Vec::with_capacity(self.items.len());
        let mut items = Vec::new();
        let mut spans = Vec::new();
        for (index, (item, span)) in std::mem::take(&mut self.items)
            .into_iter()
            .zip(std::mem::take(&mut self.spans))
            .enumerate()
        {
            if keep(Handle::new(index as u32)) {
                map.push(Some(Handle::new(items.len() as u32)));
                items.push(item);
                spans.push(span);
            } else {
                map.push(None);
            }
        }
        self.items = items;
        self.spans = spans;
        map
    }
}

impl<T> std::ops::Index<Handle<T>> for Arena<T> {
    type Output = T;

    fn index(&self, handle: Handle<T>) -> &T {
        &self.items[handle.index()]
    }
}

impl<T> std::ops::IndexMut<Handle<T>> for Arena<T> {
    fn index_mut(&mut self, handle: Handle<T>) -> &mut T {
        &mut self.items[handle.index()]
    }
}

/// An arena that interns: equal entries share one handle.
#[derive(Clone, Debug)]
pub struct UniqueArena<T: Eq + Hash + Clone> {
    arena: Arena<T>,
    lookup: HashMap<T, Handle<T>>,
}

impl<T: Eq + Hash + Clone> Default for UniqueArena<T> {
    fn default() -> Self {
        Self {
            arena: Arena::default(),
            lookup: HashMap::new(),
        }
    }
}

impl<T: Eq + Hash + Clone> PartialEq for UniqueArena<T> {
    fn eq(&self, other: &Self) -> bool {
        self.arena.items == other.arena.items
    }
}

impl<T: Eq + Hash + Clone> UniqueArena<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The handle of `item`, appended with `span` when it is new.
    pub fn insert(&mut self, item: T, span: Span) -> Handle<T> {
        if let Some(handle) = self.lookup.get(&item) {
            return *handle;
        }
        let handle = self.arena.append(item.clone(), span);
        self.lookup.insert(item, handle);
        handle
    }

    #[must_use]
    pub fn find(&self, item: &T) -> Option<Handle<T>> {
        self.lookup.get(item).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }

    #[must_use]
    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        self.arena.get(handle)
    }

    #[must_use]
    pub fn span(&self, handle: Handle<T>) -> Span {
        self.arena.span(handle)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.arena.iter()
    }

    /// Rebuilds the arena from `items` in order, as compaction does.
    pub fn rebuild(&mut self, items: Vec<(T, Span)>) {
        *self = Self::default();
        for (item, span) in items {
            let handle = self.arena.append(item.clone(), span);
            self.lookup.entry(item).or_insert(handle);
        }
    }
}

impl<T: Eq + Hash + Clone> std::ops::Index<Handle<T>> for UniqueArena<T> {
    type Output = T;

    fn index(&self, handle: Handle<T>) -> &T {
        &self.arena[handle]
    }
}
