use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{TextInstruction, parse_gpu_synchronization};

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify(instruction: &TextInstruction) -> Vec<Diagnostic> {
    match parse_gpu_synchronization(instruction) {
        Ok(_) => Vec::new(),
        Err(error) => vec![Diagnostic::error(V15, error.message).with_primary_span(error.span)],
    }
}
