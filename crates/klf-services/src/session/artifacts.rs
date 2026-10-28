use super::{ArtifactId, Session};
use std::any::Any;
use std::sync::Arc;

impl Session {
    /// Stores an immutable phase artifact for this session.
    ///
    /// # Panics
    /// Panics if more than `u32::MAX` artifacts are stored in one session.
    pub fn store_artifact<T: Any + Send + Sync>(&mut self, artifact: T) -> ArtifactId {
        let id = ArtifactId(u32::try_from(self.artifacts.len()).expect("artifact count fits u32"));
        self.artifacts.push(Arc::new(artifact));
        id
    }
    pub fn replace_artifact<T: Any + Send + Sync>(&mut self, id: ArtifactId, artifact: T) -> bool {
        let Some(slot) = self.artifacts.get_mut(id.index()) else {
            return false;
        };
        *slot = Arc::new(artifact);
        true
    }

    /// Moves an artifact out of the session, leaving its slot empty. A copy
    /// is made only when a cloned session still shares it.
    pub fn take_artifact<T: Any + Send + Sync + Clone>(&mut self, id: ArtifactId) -> Option<T> {
        let slot = self.artifacts.get_mut(id.index())?;
        if !slot.is::<T>() {
            return None;
        }
        let taken = std::mem::replace(slot, Arc::new(()));
        taken.downcast::<T>().ok().map(Arc::unwrap_or_clone)
    }

    #[must_use]
    pub fn artifact<T: Any>(&self, id: ArtifactId) -> Option<&T> {
        self.artifacts.get(id.index())?.as_ref().downcast_ref()
    }

    #[must_use]
    pub fn has_artifact(&self, id: ArtifactId) -> bool {
        self.artifacts.get(id.index()).is_some()
    }
}
