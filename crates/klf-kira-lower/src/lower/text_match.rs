//! `match` on a `String`: each arm names one or more texts, and the one
//! `else` arm takes every other value. The arms are tested in order, each
//! text by the same byte comparison `==` uses.

use super::Lowerer;
use klf_core::Name;
use klf_kira_syntax::ast::{MatchArm, PatternKind};
use klf_source::Span;
use std::collections::HashSet;

impl Lowerer<'_> {
    pub(super) fn lower_text_match(&mut self, subject: &Name, arms: &[MatchArm], span: Span) {
        let mut seen = HashSet::new();
        let mut otherwise = None;
        let mut tested = Vec::new();
        for arm in arms {
            match &arm.pattern.kind {
                PatternKind::Text(texts) => {
                    for text in texts {
                        if !seen.insert(text.as_str()) {
                            self.report_match_error(
                                arm.pattern.span,
                                format!("duplicate match text {text:?}"),
                            );
                        }
                    }
                    tested.push((arm, texts));
                }
                PatternKind::Otherwise if otherwise.is_some() => {
                    self.report_match_error(arm.pattern.span, "a match has one `else` arm");
                }
                PatternKind::Otherwise => otherwise = Some(arm),
                PatternKind::Case { name, .. } => self.report_match_error(
                    arm.pattern.span,
                    format!(
                        "a match on a `String` names texts, not the case `{}`",
                        name.name
                    ),
                ),
            }
        }
        let Some(otherwise) = otherwise else {
            self.report_match_error(span, "a match on a `String` needs an `else` arm");
            return;
        };

        let join = self.fb.new_block(&[]);
        let mut joined = false;
        for (arm, texts) in tested {
            let body = self.fb.new_block(&[]);
            for text in texts {
                let Some((literal, _)) = self.lower_string_lit(text, arm.pattern.span) else {
                    return;
                };
                let equal = self.lower_string_eq(subject.clone(), literal, false);
                let next = self.fb.new_block(&[]);
                self.fb.conditional_branch(
                    equal,
                    body.clone(),
                    Vec::new(),
                    next.clone(),
                    Vec::new(),
                );
                self.fb.switch_to(&next);
            }
            let rest = self.fb.current_block();
            joined |= self.lower_arm_body(&body, arm, &join);
            self.fb.switch_to(&rest);
        }
        let body = self.fb.new_block(&[]);
        self.fb.branch(body.clone(), Vec::new());
        joined |= self.lower_arm_body(&body, otherwise, &join);
        if joined {
            self.fb.switch_to(&join);
            self.reachable = true;
        } else {
            self.reachable = false;
        }
    }

    /// Lowers one arm into `block`, returning whether it falls through to
    /// `join`.
    fn lower_arm_body(&mut self, block: &Name, arm: &MatchArm, join: &Name) -> bool {
        self.fb.switch_to(block);
        self.reachable = true;
        self.push_scope();
        self.lower_block(&arm.body);
        self.pop_scope();
        if self.reachable {
            self.fb.branch(join.clone(), Vec::new());
            return true;
        }
        false
    }
}
