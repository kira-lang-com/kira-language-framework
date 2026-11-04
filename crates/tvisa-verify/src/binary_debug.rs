use tir_diag::Diagnostic;
use tvisa_format::TvisaImage;

pub(super) fn verify(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    image
        .validate_debug_references(memory_limit)
        .err()
        .map_or_else(Vec::new, |error| vec![error.diagnostic])
}
