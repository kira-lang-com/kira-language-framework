//! The report a trap prints (sections 18.8 and 31.4).
//!
//! A definition gives the report of the next trap it builds a byte at a time
//! through `klf.kir.block.trap_text_byte`: its code on the first line, its
//! headline on the second, then `note: ...`, `help: ...` and `label: ...`
//! lines. The trap names the span its check covers, and the report is
//! rendered against that source here, while the program compiles, so the
//! running program carries it whole. A trap given no report keeps the
//! message the span alone makes.

use super::DefinitionServiceState;
use klf_diag::report::{Place, Report};
use std::sync::Arc;

impl DefinitionServiceState {
    pub(crate) fn ll_block_trap_text_byte(&mut self, arguments: &[u64]) -> u64 {
        let Some(byte) = arguments.get(1).and_then(|byte| u8::try_from(*byte).ok()) else {
            return u64::MAX;
        };
        self.kir_trap_report.push(byte);
        0
    }

    /// The message of a trap at `start..end`: the pending report rendered
    /// against the source there, or the span's own message without one.
    pub(crate) fn trap_report(&mut self, start: Option<&u64>, end: Option<&u64>) -> Option<String> {
        let pending = std::mem::take(&mut self.kir_trap_report);
        if pending.is_empty() {
            return self.trap_message(start, end);
        }
        let text = String::from_utf8_lossy(&pending).into_owned();
        let mut lines = text.split('\n');
        let mut report = Report {
            code: lines.next().unwrap_or_default().to_owned(),
            headline: lines.next().unwrap_or_default().to_owned(),
            ..Report::default()
        };
        let mut label = String::new();
        for line in lines {
            if let Some(note) = line.strip_prefix("note: ") {
                report.notes.push(note.to_owned());
            } else if let Some(help) = line.strip_prefix("help: ") {
                report.helps.push(help.to_owned());
            } else if let Some(marked) = line.strip_prefix("label: ") {
                marked.clone_into(&mut label);
            }
        }
        report.place = self
            .span_excerpt(start, end)
            .map(|(path, source, from, to)| Place::in_text(&path, &source, from, to, &label));
        Some(report.render())
    }

    /// The path and whole text of the file a span is in, and its byte range
    /// there.
    pub(crate) fn span_excerpt(
        &self,
        start: Option<&u64>,
        end: Option<&u64>,
    ) -> Option<(String, Arc<str>, usize, usize)> {
        let raw_start = i64::from_ne_bytes((*start?).to_ne_bytes());
        let raw_end = *end?;
        if raw_start < 0 {
            let (selector, from, to) = super::source_spans::selected_span(raw_start, raw_end)?;
            let (_, path, source) = self.selected_source(selector)?;
            return Some((path.clone(), Arc::clone(source), from, to));
        }
        let (_, path, source) = self.source_entry(self.file?)?;
        Some((
            path.clone(),
            Arc::clone(source),
            usize::try_from(raw_start).ok()?,
            usize::try_from(raw_end).ok()?,
        ))
    }
}
