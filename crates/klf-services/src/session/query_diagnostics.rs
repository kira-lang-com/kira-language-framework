use super::Session;
use crate::query::QueryId;
use klf_diag::Diagnostic;
use std::collections::HashSet;

impl Session {
    /// Starts a fresh diagnostic result while retaining reusable query
    /// diagnostics for cache-hit replay.
    pub fn clear_diagnostics(&mut self) {
        self.diagnostics.clear();
        self.diagnostic_owners.clear();
    }

    pub fn report(&mut self, diagnostic: Diagnostic) {
        let owner = self.queries.active_query();
        self.push_diagnostic(diagnostic, owner, owner.is_some());
    }

    fn push_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
        owner: Option<QueryId>,
        record_for_query: bool,
    ) {
        self.diagnostics.push(diagnostic.clone());
        self.diagnostic_owners.push(owner);
        if record_for_query && let Some(owner) = owner {
            self.query_diagnostics
                .entry(owner)
                .or_default()
                .push(diagnostic);
        }
    }

    pub(crate) fn reset_query_diagnostics(&mut self, id: QueryId) {
        self.retain_diagnostics(|owner| owner != Some(id));
        self.query_diagnostics.remove(&id);
    }

    pub(crate) fn invalidate_query_diagnostics(&mut self, ids: &[QueryId]) {
        let invalidated = ids.iter().copied().collect::<HashSet<_>>();
        self.retain_diagnostics(|owner| !owner.is_some_and(|id| invalidated.contains(&id)));
        for id in ids {
            self.query_diagnostics.remove(id);
        }
    }

    fn retain_diagnostics(&mut self, mut keep: impl FnMut(Option<QueryId>) -> bool) {
        let mut diagnostics = Vec::with_capacity(self.diagnostics.len());
        let mut owners = Vec::with_capacity(self.diagnostic_owners.len());
        for (diagnostic, owner) in self
            .diagnostics
            .drain(..)
            .zip(self.diagnostic_owners.drain(..))
        {
            if keep(owner) {
                diagnostics.push(diagnostic);
                owners.push(owner);
            }
        }
        self.diagnostics = diagnostics;
        self.diagnostic_owners = owners;
    }

    pub(crate) fn replay_query_diagnostics(&mut self, id: QueryId) {
        if self.diagnostic_owners.contains(&Some(id)) {
            return;
        }
        let diagnostics = self.query_diagnostics.get(&id).cloned().unwrap_or_default();
        for diagnostic in diagnostics {
            self.push_diagnostic(diagnostic, Some(id), false);
        }
    }
}
