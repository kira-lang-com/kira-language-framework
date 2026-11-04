use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryAnalysisSummary, BinaryCodeTable, BinarySignature, BinaryType, BinaryTypeTable,
    BinaryVariantSet,
};
use tir_target::{Architecture, Platform, known_features};

const V18: Code = Code::new(Dialect::Tir, Area::Verifier, 18);

pub(super) fn verify(
    sets: &[BinaryVariantSet],
    signatures: &[BinarySignature],
    types: &BinaryTypeTable,
    code: &BinaryCodeTable,
    analysis: &BinaryAnalysisSummary,
    platform: Platform,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let defined_functions = code
        .functions
        .iter()
        .map(|function| function.function_index)
        .collect::<std::collections::HashSet<_>>();
    let member_functions = sets
        .iter()
        .flat_map(|set| &set.members)
        .map(|member| member.function_index)
        .collect::<std::collections::HashSet<_>>();

    for set in sets {
        let mut reference_signature: Option<&BinarySignature> = None;
        let mut has_assembly = false;
        for member in &set.members {
            let Some(signature) = usize::try_from(member.function_index)
                .ok()
                .and_then(|index| signatures.get(index))
            else {
                continue;
            };
            if !defined_functions.contains(&member.function_index) {
                diagnostics.push(Diagnostic::error(
                    V18,
                    format!("variant member @{} has no CODE body", signature.name),
                ));
            }
            if member.architecture != 0 {
                let Some(target) = architecture(member.architecture) else {
                    continue;
                };
                let known = known_features(target);
                for feature in &member.features {
                    if !known.contains(&feature.as_str()) {
                        diagnostics.push(Diagnostic::error(
                            V18,
                            format!(
                                "variant @{} uses unknown {} feature {feature}",
                                signature.name,
                                architecture_name(member.architecture)
                            ),
                        ));
                    }
                }
            }
            has_assembly |= analysis
                .assembly_regions
                .iter()
                .any(|region| region.function_index == member.function_index);

            if let Some(reference) = reference_signature {
                if !same_signature(reference, signature, types) {
                    diagnostics.push(Diagnostic::error(
                        V18,
                        format!(
                            "variant member @{} has a different signature from set {}",
                            signature.name, set.name
                        ),
                    ));
                }
            } else {
                reference_signature = Some(signature);
            }
        }

        let has_portable = set.members.iter().any(|member| member.architecture == 0);
        let has_wasm = set.members.iter().any(|member| member.architecture == 4);
        let has_three_bases = [1, 2, 3].into_iter().all(|architecture| {
            set.members
                .iter()
                .any(|member| member.architecture == architecture && member.features.is_empty())
        });
        if has_assembly && !has_three_bases {
            diagnostics.push(Diagnostic::error(
                V18,
                format!(
                    "variant set {} contains assembly but lacks feature-free aarch64, x64, or rv64 members",
                    set.name
                ),
            ));
        } else if !has_assembly && !has_portable && !has_three_bases {
            diagnostics.push(Diagnostic::error(
                V18,
                format!(
                    "variant set {} needs a portable member or feature-free aarch64, x64, and rv64 members",
                    set.name
                ),
            ));
        }
        if matches!(platform, Platform::Web | Platform::Wasi) {
            if has_assembly && !has_wasm {
                diagnostics.push(Diagnostic::error(
                    V18,
                    format!("variant set {} with assembly needs a wasm member", set.name),
                ));
            } else if !has_assembly && !has_portable && !has_wasm {
                diagnostics.push(Diagnostic::error(
                    V18,
                    format!(
                        "variant set {} needs a portable or wasm member for this platform",
                        set.name
                    ),
                ));
            }
        }
    }
    for (function_index, signature) in signatures.iter().enumerate() {
        if signature.variant.is_some() && !member_functions.contains(&(function_index as u64)) {
            diagnostics.push(Diagnostic::error(
                V18,
                format!(
                    "variant function @{} is not listed in a VARIANTS set",
                    signature.name
                ),
            ));
        }
    }
    for region in &analysis.assembly_regions {
        let Some(signature) = usize::try_from(region.function_index)
            .ok()
            .and_then(|index| signatures.get(index))
        else {
            continue;
        };
        if !member_functions.contains(&region.function_index) {
            diagnostics.push(Diagnostic::error(
                V18,
                format!("assembly in @{} is not in a VARIANTS set", signature.name),
            ));
        }
        if signature
            .variant
            .as_ref()
            .is_none_or(|variant| variant.architecture != region.architecture)
        {
            diagnostics.push(Diagnostic::error(
                V18,
                format!(
                    "assembly architecture does not match @{}'s variant attribute",
                    signature.name
                ),
            ));
        }
    }
    diagnostics
}

fn same_signature(
    left: &BinarySignature,
    right: &BinarySignature,
    types: &BinaryTypeTable,
) -> bool {
    let left_type = usize::try_from(left.function_type)
        .ok()
        .and_then(|index| types.entries.get(index));
    let right_type = usize::try_from(right.function_type)
        .ok()
        .and_then(|index| types.entries.get(index));
    matches!((left_type, right_type), (Some(BinaryType::Function(a)), Some(BinaryType::Function(b))) if a == b)
        && left.generic_parameters.len() == right.generic_parameters.len()
        && left
            .generic_parameters
            .iter()
            .zip(&right.generic_parameters)
            .all(|(a, b)| a.bounds == b.bounds)
}

fn architecture(code: u8) -> Option<Architecture> {
    match code {
        1 => Some(Architecture::Aarch64),
        2 => Some(Architecture::X64),
        3 => Some(Architecture::Rv64),
        4 => Some(Architecture::Wasm),
        _ => None,
    }
}

fn architecture_name(code: u8) -> &'static str {
    match code {
        1 => "aarch64",
        2 => "x64",
        3 => "rv64",
        4 => "wasm",
        _ => "portable",
    }
}
