//! Trait records and the section-8.6 local constraint solver.

use crate::Session;
use crate::types::TypeId;
use klf_core::Name;
use klf_source::Span;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TraitId(u32);

impl TraitId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Requirement {
    pub name: Name,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraitInfo {
    pub name: Name,
    pub requirements: Vec<Requirement>,
    pub parents: Vec<TraitId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conformance {
    pub witnesses: Vec<Name>,
    pub associated: Vec<TypeId>,
}

#[derive(Clone)]
pub(crate) struct TraitStore {
    traits: Vec<TraitInfo>,
    conformances: HashMap<(TypeId, TraitId), Conformance>,
    conformance_order: Vec<(TypeId, TraitId)>,
}

impl TraitStore {
    pub(crate) fn new() -> Self {
        Self {
            traits: Vec::new(),
            conformances: HashMap::new(),
            conformance_order: Vec::new(),
        }
    }

    fn declare(
        &mut self,
        name: Name,
        requirements: Vec<Requirement>,
        parents: Vec<TraitId>,
    ) -> TraitId {
        for parent in &parents {
            assert!(
                self.get(*parent).is_some(),
                "trait parent must belong to this session"
            );
        }
        let id = TraitId(u32::try_from(self.traits.len()).expect("trait count fits u32"));
        self.traits.push(TraitInfo {
            name,
            requirements,
            parents,
        });
        id
    }

    fn get(&self, id: TraitId) -> Option<&TraitInfo> {
        self.traits.get(id.index())
    }

    fn set_parents(&mut self, id: TraitId, parents: Vec<TraitId>) {
        for parent in &parents {
            assert!(
                self.get(*parent).is_some(),
                "trait parent must belong to this session"
            );
        }
        self.traits
            .get_mut(id.index())
            .unwrap_or_else(|| panic!("invalid trait {}", id.as_u32()))
            .parents = parents;
    }

    fn insert(&mut self, ty: TypeId, trait_id: TraitId, conformance: Conformance) {
        assert!(
            self.get(trait_id).is_some(),
            "conformance trait must belong to this session"
        );
        if !self.conformances.contains_key(&(ty, trait_id)) {
            self.conformance_order.push((ty, trait_id));
        }
        self.conformances.insert((ty, trait_id), conformance);
    }

    fn conformance(&self, ty: TypeId, trait_id: TraitId) -> Option<&Conformance> {
        self.conformances.get(&(ty, trait_id))
    }

    fn conformances(&self) -> impl Iterator<Item = (TypeId, TraitId, &Conformance)> {
        self.conformance_order.iter().filter_map(|(ty, trait_id)| {
            self.conformances
                .get(&(*ty, *trait_id))
                .map(|conformance| (*ty, *trait_id, conformance))
        })
    }

    fn conforms(&self, ty: TypeId, trait_id: TraitId) -> bool {
        self.conforms_inner(ty, trait_id, &mut HashSet::new())
    }

    fn conforms_inner(
        &self,
        ty: TypeId,
        trait_id: TraitId,
        visiting: &mut HashSet<TraitId>,
    ) -> bool {
        if !visiting.insert(trait_id) {
            return false;
        }
        if self.get(trait_id).is_none() {
            visiting.remove(&trait_id);
            return false;
        }
        let result = self.conformances.contains_key(&(ty, trait_id))
            || self
                .conformances
                .keys()
                .any(|(candidate_ty, candidate_trait)| {
                    *candidate_ty == ty
                        && *candidate_trait != trait_id
                        && self.extends(*candidate_trait, trait_id, &mut HashSet::new())
                });
        visiting.remove(&trait_id);
        result
    }

    fn extends(&self, child: TraitId, parent: TraitId, visiting: &mut HashSet<TraitId>) -> bool {
        if child == parent {
            return true;
        }
        if !visiting.insert(child) {
            return false;
        }
        let result = self.get(child).is_some_and(|info| {
            info.parents
                .iter()
                .any(|candidate| self.extends(*candidate, parent, visiting))
        });
        visiting.remove(&child);
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub equalities: Vec<(TypeId, TypeId)>,
    pub conformances: Vec<(TypeId, TraitId)>,
    pub cost: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub variable: TypeId,
    pub value: TypeId,
}

pub type Bindings = Vec<Binding>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conflict {
    pub message: String,
    pub origin: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Solution {
    Unique(Bindings),
    Ambiguous(Vec<Bindings>),
    Failed(Vec<Conflict>),
    TooComplex,
}

#[derive(Clone, Debug)]
enum Constraint {
    Equality {
        left: TypeId,
        right: TypeId,
        origin: Span,
    },
    Conformance {
        ty: TypeId,
        trait_id: TraitId,
        origin: Span,
    },
    Conversion {
        from: TypeId,
        to: TypeId,
        cost: i64,
        origin: Span,
    },
    Disjunction {
        choices: Vec<Choice>,
        origin: Span,
    },
}

pub struct Solver<'session> {
    session: &'session mut Session,
    variables: Vec<TypeId>,
    variable_set: HashSet<TypeId>,
    constraints: Vec<Constraint>,
}

struct SearchState {
    explored: usize,
    too_complex: bool,
    solutions: Vec<(i64, Bindings)>,
    conflicts: Vec<Conflict>,
}

impl Session {
    #[must_use]
    pub fn declare_trait(
        &mut self,
        name: Name,
        requirements: Vec<Requirement>,
        parents: Vec<TraitId>,
    ) -> TraitId {
        self.trait_store_mut().declare(name, requirements, parents)
    }

    #[must_use]
    pub fn trait_info(&self, trait_id: TraitId) -> Option<&TraitInfo> {
        self.trait_store().get(trait_id)
    }

    /// Completes the parent list of a previously declared trait.
    ///
    /// # Panics
    ///
    /// Panics if `trait_id` or one of the parent handles does not belong to
    /// this session.
    pub fn complete_trait(&mut self, trait_id: TraitId, parents: Vec<TraitId>) {
        self.trait_store_mut().set_parents(trait_id, parents);
    }

    /// Registers a direct conformance and its witness metadata.
    ///
    /// # Panics
    ///
    /// Panics if `ty` or `trait_id` does not belong to this session.
    pub fn conformance(
        &mut self,
        ty: TypeId,
        trait_id: TraitId,
        witnesses: Vec<Name>,
        associated: Vec<TypeId>,
    ) {
        assert!(
            self.type_info(ty).is_some(),
            "conformance type must be in session"
        );
        self.trait_store_mut().insert(
            ty,
            trait_id,
            Conformance {
                witnesses,
                associated,
            },
        );
    }

    #[must_use]
    pub fn conformance_info(&self, ty: TypeId, trait_id: TraitId) -> Option<&Conformance> {
        self.trait_store().conformance(ty, trait_id)
    }

    /// Returns direct conformances in registration order for deterministic
    /// definition output.
    #[must_use]
    pub fn conformance_records(&self) -> Vec<(TypeId, TraitId, Conformance)> {
        self.trait_store()
            .conformances()
            .map(|(ty, trait_id, conformance)| (ty, trait_id, conformance.clone()))
            .collect()
    }

    #[must_use]
    pub fn conforms(&self, ty: TypeId, trait_id: TraitId) -> bool {
        self.trait_store().conforms(ty, trait_id)
    }

    #[must_use]
    pub fn solver(&mut self) -> Solver<'_> {
        Solver {
            session: self,
            variables: Vec::new(),
            variable_set: HashSet::new(),
            constraints: Vec::new(),
        }
    }
}

impl Solver<'_> {
    #[must_use]
    pub fn variable(&mut self, _origin: Span) -> TypeId {
        let name = Name::new(format!("$solver{}", self.variables.len()));
        let ty = self.session.generic_type(name);
        self.variables.push(ty);
        self.variable_set.insert(ty);
        ty
    }

    pub fn equate(&mut self, left: TypeId, right: TypeId, origin: Span) {
        self.constraints.push(Constraint::Equality {
            left,
            right,
            origin,
        });
    }

    pub fn conform(&mut self, ty: TypeId, trait_id: TraitId, origin: Span) {
        self.constraints.push(Constraint::Conformance {
            ty,
            trait_id,
            origin,
        });
    }

    pub fn convertible(&mut self, from: TypeId, to: TypeId, cost: i64, origin: Span) {
        self.constraints.push(Constraint::Conversion {
            from,
            to,
            cost,
            origin,
        });
    }

    pub fn disjunction(&mut self, choices: Vec<Choice>, origin: Span) {
        self.constraints
            .push(Constraint::Disjunction { choices, origin });
    }

    #[must_use]
    pub fn solve(&mut self, limit: usize) -> Solution {
        if limit == 0 {
            return Solution::TooComplex;
        }

        let mut bindings = Vec::new();
        let mut cost = 0;
        let mut conflicts = Vec::new();
        for constraint in &self.constraints {
            if matches!(constraint, Constraint::Disjunction { .. }) {
                continue;
            }
            let previous_bindings = bindings.clone();
            let previous_cost = cost;
            if let Err(conflict) = self.apply_constraint(constraint, &mut bindings, &mut cost) {
                bindings = previous_bindings;
                cost = previous_cost;
                conflicts.push(conflict);
            }
        }
        if !conflicts.is_empty() {
            return Solution::Failed(conflicts);
        }

        let disjunctions = self
            .constraints
            .iter()
            .filter_map(|constraint| match constraint {
                Constraint::Disjunction { choices, origin } => Some((choices.clone(), *origin)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut state = SearchState {
            explored: 0,
            too_complex: false,
            solutions: Vec::new(),
            conflicts: Vec::new(),
        };
        self.explore(&disjunctions, 0, &bindings, cost, limit, &mut state);
        if state.too_complex {
            return Solution::TooComplex;
        }
        if state.solutions.is_empty() {
            conflicts.extend(state.conflicts);
            return Solution::Failed(conflicts);
        }

        let best_cost = state
            .solutions
            .iter()
            .map(|(solution_cost, _)| *solution_cost)
            .min();
        let Some(best_cost) = best_cost else {
            conflicts.extend(state.conflicts);
            return Solution::Failed(conflicts);
        };
        let mut best = Vec::new();
        for (_, bindings) in state
            .solutions
            .into_iter()
            .filter(|(solution_cost, _)| *solution_cost == best_cost)
        {
            if !best.contains(&bindings) {
                best.push(bindings);
            }
        }
        match best.as_slice() {
            [bindings] => Solution::Unique(bindings.clone()),
            _ => Solution::Ambiguous(best),
        }
    }

    fn explore(
        &self,
        disjunctions: &[(Vec<Choice>, Span)],
        index: usize,
        bindings: &Bindings,
        cost: i64,
        limit: usize,
        state: &mut SearchState,
    ) {
        if index == disjunctions.len() {
            state.solutions.push((cost, self.finish_bindings(bindings)));
            return;
        }
        let (choices, origin) = &disjunctions[index];
        if choices.is_empty() {
            state.conflicts.push(Conflict {
                message: "empty solver disjunction".to_owned(),
                origin: *origin,
            });
            return;
        }
        for choice in choices {
            if state.explored >= limit {
                state.too_complex = true;
                return;
            }
            state.explored += 1;
            let mut next_bindings = bindings.clone();
            let mut next_cost = cost;
            match self.apply_choice(choice, *origin, &mut next_bindings, &mut next_cost) {
                Ok(()) => self.explore(
                    disjunctions,
                    index + 1,
                    &next_bindings,
                    next_cost,
                    limit,
                    state,
                ),
                Err(conflict) => state.conflicts.push(conflict),
            }
            if state.too_complex {
                return;
            }
        }
    }

    fn apply_choice(
        &self,
        choice: &Choice,
        origin: Span,
        bindings: &mut Bindings,
        cost: &mut i64,
    ) -> Result<(), Conflict> {
        for (left, right) in &choice.equalities {
            self.unify(*left, *right, bindings, origin)?;
        }
        for (ty, trait_id) in &choice.conformances {
            self.check_conformance(*ty, *trait_id, bindings, origin)?;
        }
        *cost = cost.saturating_add(choice.cost);
        Ok(())
    }

    fn apply_constraint(
        &self,
        constraint: &Constraint,
        bindings: &mut Bindings,
        cost: &mut i64,
    ) -> Result<(), Conflict> {
        match constraint {
            Constraint::Equality {
                left,
                right,
                origin,
            } => self.unify(*left, *right, bindings, *origin),
            Constraint::Conformance {
                ty,
                trait_id,
                origin,
            } => self.check_conformance(*ty, *trait_id, bindings, *origin),
            Constraint::Conversion {
                from,
                to,
                cost: conversion_cost,
                origin,
            } => {
                let _ = origin;
                self.apply_conversion(*from, *to, *conversion_cost, bindings, cost);
                Ok(())
            }
            Constraint::Disjunction { .. } => Ok(()),
        }
    }

    fn apply_conversion(
        &self,
        from: TypeId,
        to: TypeId,
        conversion_cost: i64,
        bindings: &mut Bindings,
        cost: &mut i64,
    ) {
        let from = Self::resolve(from, bindings);
        let to = Self::resolve(to, bindings);
        if from != to {
            if self.variable_set.contains(&from) {
                bindings.push(Binding {
                    variable: from,
                    value: to,
                });
            } else if self.variable_set.contains(&to) {
                bindings.push(Binding {
                    variable: to,
                    value: from,
                });
            }
            *cost = cost.saturating_add(conversion_cost);
        }
    }

    fn check_conformance(
        &self,
        ty: TypeId,
        trait_id: TraitId,
        bindings: &Bindings,
        origin: Span,
    ) -> Result<(), Conflict> {
        let ty = Self::resolve(ty, bindings);
        if self.variable_set.contains(&ty) {
            return Err(Conflict {
                message: format!(
                    "cannot prove type variable {ty:?} conforms to trait {trait_id:?}"
                ),
                origin,
            });
        }
        if self.session.conforms(ty, trait_id) {
            Ok(())
        } else {
            Err(Conflict {
                message: format!("type {ty:?} does not conform to trait {trait_id:?}"),
                origin,
            })
        }
    }

    fn unify(
        &self,
        left: TypeId,
        right: TypeId,
        bindings: &mut Bindings,
        origin: Span,
    ) -> Result<(), Conflict> {
        let left = Self::resolve(left, bindings);
        let right = Self::resolve(right, bindings);
        if left == right {
            return Ok(());
        }
        if self.variable_set.contains(&left) {
            bindings.push(Binding {
                variable: left,
                value: right,
            });
            return Ok(());
        }
        if self.variable_set.contains(&right) {
            bindings.push(Binding {
                variable: right,
                value: left,
            });
            return Ok(());
        }
        Err(Conflict {
            message: format!("types {left:?} and {right:?} do not unify"),
            origin,
        })
    }

    fn resolve(mut ty: TypeId, bindings: &Bindings) -> TypeId {
        while let Some(binding) = bindings.iter().find(|binding| binding.variable == ty) {
            if binding.value == ty {
                break;
            }
            ty = binding.value;
        }
        ty
    }

    fn finish_bindings(&self, bindings: &Bindings) -> Bindings {
        self.variables
            .iter()
            .filter_map(|variable| {
                let value = Self::resolve(*variable, bindings);
                (value != *variable).then_some(Binding {
                    variable: *variable,
                    value,
                })
            })
            .collect()
    }
}
