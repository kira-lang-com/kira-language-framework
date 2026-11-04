use std::collections::HashSet;

use crate::binary_record::collect_record;
use tir_diag::{Area, Code, Diagnostic, Dialect, Severity};
use tir_format::{BinaryCapabilities, BinaryImport};
use tir_target::{Platform, Profile};

const CAP1: Code = Code::new(Dialect::Tir, Area::Capability, 1);
const CAP2: Code = Code::new(Dialect::Tir, Area::Capability, 2);
const CAP100: Code = Code::new(Dialect::Tir, Area::Capability, 100);
const PKG1: Code = Code::new(Dialect::Tir, Area::Package, 1);

pub fn verify_binary_package_capabilities(
    modules: &[tir_format::BinaryModule],
    manifest_entitlements: &[String],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut requested = manifest_entitlements.to_vec();
    requested.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    if requested.windows(2).any(|pair| pair[0] == pair[1]) {
        diagnostics.push(Diagnostic::error(
            PKG1,
            "package manifest has duplicate entitlements",
        ));
        requested.dedup();
    }

    let mut implied_by_package = HashSet::new();
    for module in modules {
        let capabilities = match module.capabilities(u64::MAX) {
            Ok(capabilities) => capabilities,
            Err(error) => {
                diagnostics.push(error.diagnostic);
                continue;
            }
        };
        let imports = match module.imports(u64::MAX) {
            Ok(imports) => imports,
            Err(error) => {
                diagnostics.push(error.diagnostic);
                continue;
            }
        };
        let module_record = collect_record(module.module_record(u64::MAX), &mut diagnostics);
        let strings = collect_record(module.string_table(u64::MAX), &mut diagnostics);
        let types = collect_record(module.type_table(u64::MAX), &mut diagnostics);
        let constants = types.as_ref().and_then(|types| {
            collect_record(module.constant_table(u64::MAX, types), &mut diagnostics)
        });
        let globals = types.as_ref().and_then(|types| {
            constants.as_ref().and_then(|constants| {
                collect_record(
                    module.global_table(u64::MAX, types, constants),
                    &mut diagnostics,
                )
            })
        });
        let signatures = types.as_ref().and_then(|types| {
            collect_record(module.signature_table(u64::MAX, types), &mut diagnostics)
        });
        let generics = if let (Some(types), Some(signatures)) = (&types, &signatures) {
            collect_record(
                module.generics_table(u64::MAX, types, signatures),
                &mut diagnostics,
            )
        } else {
            None
        };
        let code = signatures.as_ref().and_then(|signatures| {
            collect_record(module.code_table(u64::MAX, signatures), &mut diagnostics)
        });
        diagnostics.extend(verify(
            &capabilities,
            Some(&imports),
            module.header.profile,
            module.header.platform,
            (
                module_record.as_ref(),
                code.as_ref(),
                signatures.as_ref(),
                types.as_ref(),
                globals.as_ref(),
                generics.as_ref(),
                strings.as_ref(),
            ),
        ));
        if capabilities.requested_entitlements != requested {
            diagnostics.push(Diagnostic::error(
                PKG1,
                "module CAPS requested entitlements do not match the package manifest",
            ));
        }
        implied_by_package.extend(capabilities.implied_entitlements);
    }

    for entitlement in requested {
        if !implied_by_package.contains(&entitlement) {
            diagnostics.push(Diagnostic::new(
                Severity::Remark,
                CAP100,
                format!("requested entitlement `{entitlement}` is unused"),
            ));
        }
    }
    diagnostics
}

pub fn recompute_binary_module_capabilities(
    module: &tir_format::BinaryModule,
    manifest_entitlements: &[String],
) -> Result<BinaryCapabilities, tir_diag::Diagnostic> {
    let imports = module.imports(u64::MAX).map_err(|error| error.diagnostic)?;
    let strings = module
        .string_table(u64::MAX)
        .map_err(|error| error.diagnostic)?;
    let types = module
        .type_table(u64::MAX)
        .map_err(|error| error.diagnostic)?;
    let constants = module
        .constant_table(u64::MAX, &types)
        .map_err(|error| error.diagnostic)?;
    let globals = module
        .global_table(u64::MAX, &types, &constants)
        .map_err(|error| error.diagnostic)?;
    let signatures = module
        .signature_table(u64::MAX, &types)
        .map_err(|error| error.diagnostic)?;
    let generics = module
        .generics_table(u64::MAX, &types, &signatures)
        .map_err(|error| error.diagnostic)?;
    let code = module
        .code_table(u64::MAX, &signatures)
        .map_err(|error| error.diagnostic)?;
    let record = module
        .module_record(u64::MAX)
        .map_err(|error| error.diagnostic)?;
    let mut capabilities = if module.sections.iter().any(|section| section.kind == 13) {
        module
            .capabilities(u64::MAX)
            .map_err(|error| error.diagnostic)?
    } else {
        BinaryCapabilities::default()
    };
    let mut requested = manifest_entitlements.to_vec();
    requested.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    requested.dedup();
    capabilities.requested_entitlements = requested;
    let reached = crate::capability_calls::compute_reached_imports(
        (
            Some(&record),
            Some(&code),
            Some(&signatures),
            Some(&types),
            Some(&globals),
            Some(&generics),
            Some(&strings),
        ),
        &imports,
        module.header.platform,
    );
    capabilities.reached_libtessera_calls = reached.libtessera_calls;
    capabilities.reached_requires_functions = reached.requires_functions;
    capabilities.reached_platform_imports = reached.platform_imports;
    capabilities.implied_entitlements = reached.implied_entitlements;
    Ok(capabilities)
}

pub(super) fn verify(
    capabilities: &BinaryCapabilities,
    imports: Option<&[BinaryImport]>,
    profile: Profile,
    platform: Platform,
    call_graph: super::capability_calls::CallGraphRecords<'_>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let import_records = imports;
    let imports = imports.unwrap_or_default();

    for reference in &capabilities.reached_libtessera_calls {
        let Some(import) = usize::try_from(reference.import_index)
            .ok()
            .and_then(|index| imports.get(index))
        else {
            continue;
        };
        if !super::platform::is_libtessera_module(&import.name) {
            diagnostics.push(Diagnostic::error(
                CAP1,
                format!(
                    "CAPS LibTessera call references non-LibTessera module `{}`",
                    import.name
                ),
            ));
        }
    }

    for import in imports {
        for entity in &import.entities {
            if !entity.implied_entitlements.is_empty()
                && (import.kind != tir_format::BinaryImportKind::Ridl
                    || !super::platform::is_libtessera_module(&import.name))
            {
                diagnostics.push(Diagnostic::error(
                    CAP1,
                    format!(
                        "LibTessera implied entitlements are attached to non-LibTessera import `{}`",
                        import.name
                    ),
                ));
            }
        }
    }

    if platform != Platform::Tessera && !capabilities.implied_entitlements.is_empty() {
        diagnostics.push(Diagnostic::error(
            CAP2,
            "non-Tessera platform modules cannot imply entitlements",
        ));
    }
    if profile == Profile::Kernel
        && (!capabilities.requested_entitlements.is_empty()
            || !capabilities.implied_entitlements.is_empty())
    {
        diagnostics.push(Diagnostic::error(
            CAP2,
            "kernel profile does not allow entitlements",
        ));
    }

    for entitlement in &capabilities.implied_entitlements {
        if capabilities
            .requested_entitlements
            .binary_search_by(|requested| requested.as_bytes().cmp(entitlement.as_bytes()))
            .is_err()
        {
            diagnostics.push(Diagnostic::error(
                CAP1,
                format!("implied entitlement `{entitlement}` was not requested"),
            ));
        }
    }

    diagnostics.extend(crate::capability_calls::verify_reached_imports(
        call_graph,
        import_records,
        capabilities,
        platform,
    ));

    diagnostics
}
