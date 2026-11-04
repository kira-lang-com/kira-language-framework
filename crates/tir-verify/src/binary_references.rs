use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCopyPolicy, BinaryDestroyPolicy, BinaryImport, BinaryImportEntityKind,
    BinaryModuleRecord, BinaryRelocatePolicy, BinarySignatureTable, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_binary_references(
    module: &BinaryModuleRecord,
    imports: &[BinaryImport],
    types: &BinaryTypeTable,
    globals: Option<&tir_format::BinaryGlobalTable>,
    profile: tir_target::Profile,
    signatures: &BinarySignatureTable,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for initializer in module.initializers.iter().chain(&module.finalizers) {
        let Some(signature) = signature_at(signatures, initializer.function_index) else {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "module initializer/finalizer function index {} is out of range",
                    initializer.function_index
                ),
            ));
            continue;
        };
        if signature.entry_kind.is_some() {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "module initializer/finalizer {} cannot be a GPU entry",
                    signature.name
                ),
            ));
        }
    }

    for import in imports {
        for entity in &import.entities {
            let Some(ty) = types
                .entries
                .get(usize::try_from(entity.type_index).unwrap_or(usize::MAX))
            else {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "import {}.{} has an out-of-range type index",
                        import.name, entity.name
                    ),
                ));
                continue;
            };
            if entity.kind == BinaryImportEntityKind::Function
                && !matches!(ty, BinaryType::Function(_))
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function import {}.{} does not have a function type",
                        import.name, entity.name
                    ),
                ));
            }
        }
    }

    for definition in &types.definitions {
        for index in policy_functions(&definition.policy) {
            if signature_at(signatures, index).is_none() {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "type declaration {} policy function index {index} is out of range",
                        definition.declaration
                    ),
                ));
            }
        }
    }

    if let Some(globals) = globals {
        for global in &globals.records {
            if global.per_cpu && profile != tir_target::Profile::Kernel {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "per-CPU global {} is only valid in the kernel profile",
                        global.name
                    ),
                ));
            }
        }
    }

    diagnostics
}

fn signature_at(
    signatures: &BinarySignatureTable,
    index: u64,
) -> Option<&tir_format::BinarySignature> {
    signatures.records.get(usize::try_from(index).ok()?)
}

fn policy_functions(policy: &tir_format::BinaryPolicy) -> Vec<u64> {
    let mut functions = Vec::new();
    match policy.copy {
        BinaryCopyPolicy::Implicit(index) | BinaryCopyPolicy::Explicit(index) => {
            functions.push(index);
        }
        BinaryCopyPolicy::Trivial | BinaryCopyPolicy::None => {}
    }
    if let BinaryDestroyPolicy::Function(index) = policy.destroy {
        functions.push(index);
    }
    if let BinaryRelocatePolicy::Function(index) = policy.relocate {
        functions.push(index);
    }
    functions
}
