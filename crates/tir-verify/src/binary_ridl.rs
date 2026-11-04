use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryGlobalTable, BinaryLinkage, BinaryRidlExportKind, BinaryRidlTable, BinarySignatureTable,
    BinaryType, BinaryTypeTable,
};

const V20: Code = Code::new(Dialect::Tir, Area::Verifier, 20);
const V12: Code = Code::new(Dialect::Tir, Area::Verifier, 12);

pub(super) fn verify_exports(
    ridl: Option<&BinaryRidlTable>,
    signatures: Option<&BinarySignatureTable>,
    globals: Option<&BinaryGlobalTable>,
    types: Option<&BinaryTypeTable>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if let (Some(signatures), Some(types)) = (signatures, types) {
        for signature in signatures
            .records
            .iter()
            .filter(|signature| signature.linkage == BinaryLinkage::Exported)
        {
            let function_type = usize::try_from(signature.function_type)
                .ok()
                .and_then(|index| types.entries.get(index));
            if matches!(function_type, Some(BinaryType::Function(function)) if function.c_abi) {
                diagnostics.push(Diagnostic::error(
                    V12,
                    format!(
                        "exported function @{} must use the RIDL ABI",
                        signature.name
                    ),
                ));
            }
        }
    }
    let (Some(ridl), Some(signatures), Some(globals)) = (ridl, signatures, globals) else {
        return diagnostics;
    };
    let has_exports = signatures
        .records
        .iter()
        .any(|signature| signature.linkage == BinaryLinkage::Exported || signature.inlinable)
        || globals
            .records
            .iter()
            .any(|global| global.linkage == BinaryLinkage::Exported);
    if ridl.canonical_hash.is_none() {
        if has_exports {
            diagnostics.push(Diagnostic::error(
                V20,
                "RIDL section is missing from a module with exported entities",
            ));
        }
        return diagnostics;
    }

    for (index, signature) in signatures.records.iter().enumerate() {
        let exported_by_ridl = ridl.exports.iter().any(|export| {
            export.entity_index == index as u64
                && matches!(
                    export.kind,
                    BinaryRidlExportKind::Function
                        | BinaryRidlExportKind::Method
                        | BinaryRidlExportKind::Instance
                )
        });
        if (signature.linkage == BinaryLinkage::Exported) != exported_by_ridl {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "RIDL export records do not match function linkage for @{}",
                    signature.name
                ),
            ));
        }
    }
    for (index, global) in globals.records.iter().enumerate() {
        let exported_by_ridl = ridl.exports.iter().any(|export| {
            export.kind == BinaryRidlExportKind::Global && export.entity_index == index as u64
        });
        if (global.linkage == BinaryLinkage::Exported) != exported_by_ridl {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "RIDL export records do not match global linkage for @{}",
                    global.name
                ),
            ));
        }
    }
    for export in &ridl.exports {
        let valid = match export.kind {
            BinaryRidlExportKind::Function
            | BinaryRidlExportKind::Method
            | BinaryRidlExportKind::Instance => signatures
                .records
                .get(export.entity_index as usize)
                .is_some_and(|signature| signature.linkage == BinaryLinkage::Exported),
            BinaryRidlExportKind::Global => globals
                .records
                .get(export.entity_index as usize)
                .is_some_and(|global| global.linkage == BinaryLinkage::Exported),
            BinaryRidlExportKind::Descriptor
            | BinaryRidlExportKind::Builder
            | BinaryRidlExportKind::DescriptorSize
            | BinaryRidlExportKind::Witness => true,
        };
        if !valid {
            diagnostics.push(Diagnostic::error(
                V20,
                format!(
                    "RIDL export {} does not name an exported entity",
                    export.symbol
                ),
            ));
        }
    }
    diagnostics
}
