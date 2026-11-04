use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::{
    BinaryAnalysisSummary, BinaryCodeTable, BinaryEngine, BinaryModuleRecord, BinarySignatureTable,
    BinaryTarget, SemanticsMode,
};

const ENCODING: Code = Code::new(Dialect::Tir, Area::Encoding, 1);

const DEFINED_ONLY: u32 = 1 << 0;
const HAS_GPU_CODE: u32 = 1 << 1;
const HAS_UNMANAGED_FUNCTIONS: u32 = 1 << 2;
const HAS_ASSEMBLY: u32 = 1 << 3;
const HAS_RUNTIME_FUNCTIONS: u32 = 1 << 4;
const HAS_SANDBOXED_FUNCTIONS: u32 = 1 << 5;

pub(super) fn verify(
    flags: u32,
    module: Option<&BinaryModuleRecord>,
    signatures: Option<&BinarySignatureTable>,
    code: Option<&BinaryCodeTable>,
    analysis: Option<&BinaryAnalysisSummary>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let (Some(module), Some(signatures)) = (module, signatures) {
        let defined_only = signatures.records.iter().all(|signature| {
            signature.semantics.unwrap_or(module.semantics) == SemanticsMode::Defined
        });
        check_flag(
            &mut diagnostics,
            flags,
            DEFINED_ONLY,
            defined_only,
            "defined-only",
        );
        check_flag(
            &mut diagnostics,
            flags,
            HAS_RUNTIME_FUNCTIONS,
            signatures
                .records
                .iter()
                .any(|signature| signature.engine == Some(BinaryEngine::Runtime)),
            "runtime-engine functions",
        );
    }

    if let (Some(signatures), Some(code)) = (signatures, code) {
        let has_gpu_code = code.functions.iter().any(|function| {
            usize::try_from(function.function_index)
                .ok()
                .and_then(|index| signatures.records.get(index))
                .is_some_and(|signature| {
                    matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both)
                })
        });
        check_flag(
            &mut diagnostics,
            flags,
            HAS_GPU_CODE,
            has_gpu_code,
            "GPU code",
        );
    }

    if let Some(analysis) = analysis {
        check_flag(
            &mut diagnostics,
            flags,
            HAS_UNMANAGED_FUNCTIONS,
            analysis
                .functions
                .iter()
                .any(|function| function.trust_class == 2),
            "unmanaged functions",
        );
        check_flag(
            &mut diagnostics,
            flags,
            HAS_ASSEMBLY,
            !analysis.assembly_regions.is_empty(),
            "assembly",
        );
        check_flag(
            &mut diagnostics,
            flags,
            HAS_SANDBOXED_FUNCTIONS,
            analysis
                .functions
                .iter()
                .any(|function| function.trust_class == 1),
            "sandboxed functions",
        );
    }

    diagnostics
}

fn check_flag(
    diagnostics: &mut Vec<Diagnostic>,
    flags: u32,
    flag: u32,
    expected: bool,
    description: &str,
) {
    if (flags & flag != 0) != expected {
        diagnostics.push(
            Diagnostic::error(
                ENCODING,
                format!("header {description} flag disagrees with module records"),
            )
            .with_primary_span(Span::new(8, 12)),
        );
    }
}
