//! `klf.kir.function.at`: where the instructions a definition builds next
//! come from, as the file, line and column of a span (section 16.3). The
//! backend turns them into the line table a debugger and a backtrace read.
//!
//! A lowering names a place for nearly every expression, so the line each
//! offset falls on is found in a table of line starts, built once per file.

use super::DefinitionServiceState;
use klf_kir::DebugLocation;
use std::sync::Arc;

impl DefinitionServiceState {
    pub(crate) fn ll_function_at(&mut self, arguments: &[u64]) -> u64 {
        let Some(function) = arguments.first().and_then(|id| usize::try_from(*id).ok()) else {
            return u64::MAX;
        };
        let location = self.debug_location(arguments.get(1), arguments.get(2));
        let Some(Some(function)) = self.functions.get_mut(function) else {
            return u64::MAX;
        };
        // The line table goes by line: a place on the line the code already
        // comes from changes nothing, and keeps the function's KIR small.
        let Some(location) = location else {
            return 0;
        };
        let same_line = function
            .builder
            .location()
            .is_some_and(|current| current.line == location.line && current.file == location.file);
        if !same_line {
            function.builder.at(Some(location));
        }
        0
    }

    /// The file, line and column the span starts at.
    fn debug_location(&mut self, start: Option<&u64>, end: Option<&u64>) -> Option<DebugLocation> {
        let (path, text, offset, _) = self.span_excerpt(start, end)?;
        let starts = match self.line_starts.get(&path) {
            Some(starts) => Arc::clone(starts),
            None => {
                let starts = Arc::new(line_starts(&text));
                self.line_starts.insert(path.clone(), Arc::clone(&starts));
                starts
            }
        };
        let offset = offset.min(text.len());
        let line = starts.partition_point(|start| *start <= offset).max(1);
        let line_start = starts[line - 1];
        let column = text.get(line_start..offset).map_or(1, |prefix| prefix.chars().count() + 1);
        Some(DebugLocation {
            file: path,
            line: u32::try_from(line).ok()?,
            column: u32::try_from(column).ok()?,
        })
    }
}

/// The byte offset each line of `text` starts at, the first line's 0.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(newline, _)| newline + 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::line_starts;

    #[test]
    fn every_line_start_is_listed() {
        assert_eq!(line_starts("a\nbc\n\nd"), vec![0, 2, 5, 6]);
    }
}
