use super::lists::is_aggregate;
use super::{Access, Local, Lowerer};
use crate::enums::{EnumCase, EnumInfo};
use crate::ty::FeType;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{BlockTarget, Type};
use klf_kira_syntax::ast::{Expr, MatchArm, PatternKind};
use klf_source::Span;
use std::collections::HashSet;

/// An arm whose pattern checked out: the block its body goes in, the block
/// parameter that receives the payload address, and the payload types its
/// bindings take in order.
struct PreparedArm<'a> {
    arm: &'a MatchArm,
    block: Name,
    payload: Option<Name>,
    types: Vec<FeType>,
}

impl Lowerer<'_> {
    pub(super) fn lower_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span) {
        let Some((value, scrutinee_ty)) = self.lower_expr(scrutinee) else {
            return;
        };
        // The arms bind parts of the value in place, so the match holds a copy
        // of anything an arm could overwrite or move. A place inside a
        // parameter the body cannot write is matched where it is, and its
        // parts are bound as aliases, which a move copies out of.
        let shared = self.shared_root(scrutinee);
        let value = if !shared
            && !self.is_temporary(&value)
            && (self.counted(&scrutinee_ty) || super::ownership::reads_through_index(scrutinee))
        {
            self.temporary_copy(value, &scrutinee_ty)
        } else {
            value
        };
        if super::expr::is_string(&scrutinee_ty) {
            self.lower_text_match(&value, arms, span);
            return;
        }
        let FeType::Enum(enum_name) = scrutinee_ty else {
            self.report_match_error(span, "match requires an enum or `String` value");
            return;
        };
        let Some(info) = self.enums.get(&enum_name).cloned() else {
            self.report_match_error(span, format!("unknown enum `{enum_name}`"));
            return;
        };

        let mut seen = HashSet::new();
        let mut cases = Vec::new();
        let mut lowered_arms = Vec::new();
        for arm in arms {
            let Some((variant, case)) = self.check_arm(arm, &info, &enum_name, &mut seen) else {
                continue;
            };
            let stored = case.stored(&enum_name);
            let payload = stored.as_ref().map(|_| self.fb.fresh_value());
            let params: Vec<(Name, Type)> = payload
                .iter()
                .zip(&stored)
                .map(|(name, ty)| (name.clone(), Type::Address(Box::new(ty.to_kir()))))
                .collect();
            let block = self.fb.new_block(&params);
            let arguments = if payload.is_some() {
                vec![self.fb.enum_payload_addr(value.clone(), variant)]
            } else {
                Vec::new()
            };
            cases.push((
                variant,
                BlockTarget {
                    block: block.clone(),
                    arguments,
                },
            ));
            lowered_arms.push(PreparedArm {
                arm,
                block,
                payload,
                types: case.payload,
            });
        }

        if seen.len() != info.variants.len() {
            self.report_match_error(span, "match is not exhaustive");
        }
        if cases.is_empty() {
            self.trap_reported(
                span,
                &super::trap_report::TrapText {
                    code: "KLF-RT-0001",
                    headline: "a `match` with no arm that applies ran",
                    label: "this match",
                    note: "the match lists no arm for any case of its enum",
                    help: "give the match an arm for every case",
                },
            );
            self.reachable = false;
            return;
        }

        let join = self.fb.new_block(&[]);
        self.fb.switch_enum(value, cases, None);
        self.reachable = false;
        let joined = self.emit_arm_bodies(lowered_arms, &join, shared);
        if joined {
            self.fb.switch_to(&join);
            self.reachable = true;
        } else {
            self.reachable = false;
        }
    }

    /// Checks one arm's pattern against the enum and returns its variant index
    /// and case, or `None` after reporting why the arm cannot lower.
    fn check_arm(
        &mut self,
        arm: &MatchArm,
        info: &EnumInfo,
        enum_name: &Name,
        seen: &mut HashSet<u32>,
    ) -> Option<(u32, EnumCase)> {
        let PatternKind::Case { name, bindings } = &arm.pattern.kind else {
            self.report_match_error(
                arm.pattern.span,
                format!("a match on `{enum_name}` names its cases, not texts"),
            );
            return None;
        };
        let Some((variant, case)) = info.variant(name.name.as_str()) else {
            self.report_match_error(
                arm.pattern.span,
                format!("`{}` is not a case of `{enum_name}`", name.name),
            );
            return None;
        };
        if !seen.insert(variant) {
            self.report_match_error(
                arm.pattern.span,
                format!("duplicate match case `{}`", name.name),
            );
            return None;
        }
        if !bindings.is_empty() && bindings.len() != case.payload.len() {
            self.report_match_error(
                arm.pattern.span,
                format!(
                    "case `{}` expects {} payload bindings, got {}",
                    case.name,
                    case.payload.len(),
                    bindings.len()
                ),
            );
            return None;
        }
        Some((variant, case))
    }

    /// Lowers each arm body into its block. Returns whether any of them fell
    /// through to the join block.
    fn emit_arm_bodies(&mut self, arms: Vec<PreparedArm<'_>>, join: &Name, shared: bool) -> bool {
        let mut joined = false;
        for PreparedArm {
            arm,
            block,
            payload,
            types,
        } in arms
        {
            self.fb.switch_to(&block);
            self.reachable = true;
            self.push_scope();
            if let Some(payload) = payload {
                let places = self.payload_places(&payload, types.len());
                let bindings = match &arm.pattern.kind {
                    PatternKind::Case { bindings, .. } => bindings.as_slice(),
                    PatternKind::Text(_) | PatternKind::Otherwise => &[],
                };
                for ((binding, ty), place) in bindings.iter().zip(types).zip(places) {
                    let access = if is_aggregate(&ty) {
                        Access::Ssa(place)
                    } else {
                        Access::Slot(place)
                    };
                    self.bind(
                        binding.name.clone(),
                        Local {
                            access,
                            ty,
                            mutable: false,
                            shared: shared.then_some(super::Shared::Alias),
                        },
                    );
                }
            }
            self.lower_block(&arm.body);
            self.pop_scope();
            if self.reachable {
                self.fb.branch(join.clone(), Vec::new());
                joined = true;
            }
        }
        joined
    }

    pub(super) fn report_match_error(&mut self, span: Span, message: impl Into<String>) {
        self.session
            .report(Diagnostic::error("KLFK-E0206", message).with_label(span, "in this match"));
    }
}
