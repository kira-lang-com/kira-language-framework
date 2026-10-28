//! The report a trap in a definition prints (KLF-RIDL sections 18.8 and
//! 31.4): rendered here, from the definition's source, so the running
//! definition carries it whole, and given the values the check compared
//! through `klf.trap.operands` as it fails.

use super::Lowerer;
use klf_core::Name;
use klf_diag::report::{Place, Report};
use klf_source::Span;

/// What a failed check says, before its place is known.
pub(crate) struct TrapText<'a> {
    pub code: &'a str,
    pub headline: &'a str,
    pub label: &'a str,
    pub note: &'a str,
    pub help: &'a str,
}

impl Lowerer<'_> {
    /// `text` rendered at `span` of the definition's source.
    pub(crate) fn trap_report(&self, span: Span, text: &TrapText<'_>) -> String {
        let file = self.session.file(span.file);
        Report {
            code: text.code.to_owned(),
            headline: text.headline.to_owned(),
            place: Some(Place::in_text(
                file.name().as_str(),
                file.text(),
                span.start as usize,
                span.end as usize,
                text.label,
            )),
            notes: vec![text.note.to_owned()],
            helps: vec![text.help.to_owned()],
        }
        .render()
    }

    /// Continues when `condition` holds, and otherwise records `operands`
    /// for the report and traps with it.
    pub(crate) fn trap_unless_reported(
        &mut self,
        condition: Name,
        span: Span,
        text: &TrapText<'_>,
        operands: [Name; 2],
    ) {
        let pass = self.fb.new_block(&[]);
        let fail = self.fb.new_block(&[]);
        self.fb
            .conditional_branch(condition, pass.clone(), Vec::new(), fail.clone(), Vec::new());
        self.fb.switch_to(&fail);
        self.fb
            .call_foreign_void(Name::new("klfsvc_trap_operands"), operands.to_vec());
        let message = self.trap_report(span, text);
        self.fb.trap(message);
        self.fb.switch_to(&pass);
    }

    /// Traps here with `text` at `span`.
    pub(crate) fn trap_reported(&mut self, span: Span, text: &TrapText<'_>) {
        let message = self.trap_report(span, text);
        self.fb.trap(message);
    }
}
