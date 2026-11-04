use tir_diag::Diagnostic;

pub(crate) fn collect_record<T>(
    result: Result<T, tir_format::FormatError>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<T> {
    match result {
        Ok(record) => Some(record),
        Err(error) => {
            diagnostics.push(error.diagnostic);
            None
        }
    }
}
