use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryAnalysisSummary, BinaryEngine, BinarySignatureTable, BinaryTarget, SemanticsMode,
};

const ENCODING: Code = Code::new(Dialect::Tir, Area::Encoding, 1);

/// Checks that the header's summary flags agree with the decoded module tables.
pub(super) fn verify(
    flags: u32,
    module_semantics: SemanticsMode,
    signatures: &BinarySignatureTable,
    analysis: &BinaryAnalysisSummary,
) -> Vec<Diagnostic> {
    let summaries = [
        (
            0,
            "defined-only",
            signatures.records.iter().all(|signature| {
                signature.semantics.unwrap_or(module_semantics) == SemanticsMode::Defined
            }),
        ),
        (
            1,
            "GPU code",
            signatures
                .records
                .iter()
                .any(|signature| signature.target != BinaryTarget::Cpu),
        ),
        (
            2,
            "unmanaged functions",
            analysis
                .functions
                .iter()
                .any(|function| function.trust_class == 2),
        ),
        (3, "assembly", !analysis.assembly_regions.is_empty()),
        (
            4,
            "runtime-engine functions",
            signatures
                .records
                .iter()
                .any(|signature| signature.engine == Some(BinaryEngine::Runtime)),
        ),
        (
            5,
            "sandboxed functions",
            analysis
                .functions
                .iter()
                .any(|function| function.trust_class == 1),
        ),
    ];

    summaries
        .into_iter()
        .filter_map(|(bit, name, expected)| {
            let actual = flags & (1 << bit) != 0;
            (actual != expected).then(|| {
                Diagnostic::error(
                    ENCODING,
                    format!(
                        "TIR header {name} flag is {}, but the module tables require it to be {}",
                        if actual { "set" } else { "clear" },
                        if expected { "set" } else { "clear" },
                    ),
                )
            })
        })
        .collect()
}
