//! Session-owned typed arenas from section 8.3.

use crate::Session;
use klf_core::Name;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ArenaId(u32);

impl ArenaId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Id<T> {
    arena: ArenaId,
    index: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    #[must_use]
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Arena<T> {
    id: ArenaId,
    marker: PhantomData<fn() -> T>,
}

struct ErasedArena {
    type_id: TypeId,
    values: Vec<Box<dyn Any>>,
}

enum ArenaUndo {
    Created {
        name: Name,
        id: ArenaId,
    },
    Allocated {
        id: ArenaId,
    },
    Set {
        id: ArenaId,
        index: usize,
        value: Box<dyn Any>,
    },
}

pub(crate) struct ArenaStore {
    names: HashMap<Name, ArenaId>,
    arenas: Vec<ErasedArena>,
    undo: Vec<ArenaUndo>,
    markers: Vec<usize>,
}

impl ArenaStore {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            names: HashMap::new(),
            arenas: Vec::new(),
            undo: Vec::new(),
            markers: Vec::new(),
        }
    }

    pub(crate) fn begin_speculation(&mut self) -> usize {
        let marker = self.undo.len();
        self.markers.push(marker);
        marker
    }

    pub(crate) fn commit_speculation(&mut self, marker: usize) {
        assert_eq!(self.markers.pop(), Some(marker), "speculation markers nest");
        if self.markers.is_empty() {
            self.undo.clear();
        }
    }

    pub(crate) fn rollback_speculation(&mut self, marker: usize) {
        assert_eq!(self.markers.pop(), Some(marker), "speculation markers nest");
        while self.undo.len() > marker {
            match self.undo.pop().expect("undo entry exists") {
                ArenaUndo::Created { name, id } => {
                    assert_eq!(id, ArenaId(u32::try_from(self.arenas.len() - 1).unwrap()));
                    self.arenas.pop();
                    self.names.remove(&name);
                }
                ArenaUndo::Allocated { id } => {
                    self.arenas[id.index()]
                        .values
                        .pop()
                        .expect("allocated value exists");
                }
                ArenaUndo::Set { id, index, value } => {
                    self.arenas[id.index()].values[index] = value;
                }
            }
        }
        if self.markers.is_empty() {
            self.undo.clear();
        }
    }

    pub(crate) fn get_or_create<T: Any>(&mut self, name: Name) -> Arena<T> {
        if let Some(id) = self.names.get(&name).copied() {
            self.assert_type(id, TypeId::of::<T>());
            return Arena {
                id,
                marker: PhantomData,
            };
        }
        let id = ArenaId(u32::try_from(self.arenas.len()).expect("arena count fits u32"));
        if self.speculating() {
            self.undo.push(ArenaUndo::Created {
                name: name.clone(),
                id,
            });
        }
        self.names.insert(name, id);
        self.arenas.push(ErasedArena {
            type_id: TypeId::of::<T>(),
            values: Vec::new(),
        });
        Arena {
            id,
            marker: PhantomData,
        }
    }

    fn assert_type(&self, id: ArenaId, expected: TypeId) {
        let actual = self
            .arenas
            .get(id.index())
            .unwrap_or_else(|| panic!("invalid arena {}", id.0))
            .type_id;
        assert_eq!(
            actual, expected,
            "arena requested with a different element type"
        );
    }

    fn alloc<T: Any>(&mut self, arena: &Arena<T>, value: T) -> Id<T> {
        self.assert_type(arena.id, TypeId::of::<T>());
        let speculating = self.speculating();
        let slot = self
            .arenas
            .get_mut(arena.id.index())
            .expect("arena was validated above");
        let index = u32::try_from(slot.values.len()).expect("arena element count fits u32");
        slot.values.push(Box::new(value));
        if speculating {
            self.undo.push(ArenaUndo::Allocated { id: arena.id });
        }
        Id {
            arena: arena.id,
            index,
            marker: PhantomData,
        }
    }

    fn get<T: Any>(&self, arena: &Arena<T>, id: Id<T>) -> Option<&T> {
        self.assert_type(arena.id, TypeId::of::<T>());
        if id.arena != arena.id {
            return None;
        }
        self.arenas
            .get(arena.id.index())?
            .values
            .get(id.index())?
            .downcast_ref()
    }

    fn set<T: Any>(&mut self, arena: &Arena<T>, id: Id<T>, value: T) {
        self.assert_type(arena.id, TypeId::of::<T>());
        assert_eq!(id.arena, arena.id, "id belongs to another arena");
        let speculating = self.speculating();
        let index = id.index();
        let slot = self
            .arenas
            .get_mut(arena.id.index())
            .expect("arena was validated above");
        let entry = slot
            .values
            .get_mut(index)
            .expect("id belongs to this arena");
        let old = std::mem::replace(entry, Box::new(value));
        if speculating {
            self.undo.push(ArenaUndo::Set {
                id: arena.id,
                index,
                value: old,
            });
        }
    }

    fn count<T: Any>(&self, arena: &Arena<T>) -> usize {
        self.assert_type(arena.id, TypeId::of::<T>());
        self.arenas[arena.id.index()].values.len()
    }

    fn speculating(&self) -> bool {
        !self.markers.is_empty()
    }
}

impl<T: Any> Arena<T> {
    /// Allocates a value and returns its typed arena id.
    pub fn alloc(self, session: &mut Session, value: T) -> Id<T> {
        session.arena_store_mut().alloc(&self, value)
    }

    /// Reads a value by id. The reference is valid for the borrow of `session`.
    #[must_use]
    pub fn get(self, session: &Session, id: Id<T>) -> Option<&T> {
        session.arena_store().get(&self, id)
    }

    /// Replaces a value already allocated in this arena.
    ///
    /// # Panics
    /// Panics if `id` belongs to another arena or is not allocated.
    pub fn set(self, session: &mut Session, id: Id<T>, value: T) {
        session.arena_store_mut().set(&self, id, value);
    }

    #[must_use]
    pub fn count(self, session: &Session) -> usize {
        session.arena_store().count(&self)
    }
}
