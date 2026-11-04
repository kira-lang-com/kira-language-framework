use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use tir_format::{BinaryImportEntityKind, BinaryImportKind};

use super::super::package::PackageReader;
use super::ModuleData;

mod artifacts;
use artifacts::{gpu_report, ridl_report, string_report};

pub(in crate::tirc) fn create(
    package: &PackageReader,
    modules: &[ModuleData],
) -> Result<Value, String> {
    let profile = package
        .manifest()
        .get("profile")
        .and_then(Value::as_str)
        .ok_or_else(|| "manifest profile is missing or not a string".to_owned())?;
    let mut manifest_requested = package
        .manifest()
        .get("entitlements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    manifest_requested.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    manifest_requested.dedup();
    let mut languages = BTreeSet::new();
    let mut producers = Vec::new();
    for module in modules {
        if let Some(producer) = &module.record.producer {
            let mut producer_languages = producer.languages.clone();
            producer_languages.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            producer_languages.dedup();
            languages.extend(producer_languages.iter().cloned());
            producers.push(json!({
                "module": module.record.name,
                "name": producer.name,
                "version": producer.version,
                "languages": producer_languages,
            }));
        }
    }
    producers.sort_by(|left, right| left["module"].as_str().cmp(&right["module"].as_str()));

    let capabilities = capability_report(modules, &manifest_requested)?;
    let trust = trust_report(modules);
    let semantics = semantics_report(modules);
    let assembly = assembly_report(modules)?;
    let variants = variant_report(modules);
    let dynamic_code = dynamic_code_report(modules, &manifest_requested);
    let gpu = gpu_report(modules);
    let ridl = ridl_report(modules);
    let strings = string_report(modules);

    let mut report = Map::new();
    report.insert("profile".to_owned(), json!(profile));
    report.insert(
        "languages".to_owned(),
        json!(languages.into_iter().collect::<Vec<_>>()),
    );
    report.insert("producers".to_owned(), json!(producers));
    report.insert("capabilities".to_owned(), capabilities);
    report.insert("trust".to_owned(), trust);
    report.insert("semantics".to_owned(), semantics);
    report.insert("assembly".to_owned(), json!(assembly));
    report.insert("variants".to_owned(), json!(variants));
    report.insert("dynamicCode".to_owned(), dynamic_code);
    report.insert("gpu".to_owned(), json!(gpu));
    report.insert("ridl".to_owned(), json!(ridl));
    report.insert("strings".to_owned(), json!(strings));
    report.insert(
        "license".to_owned(),
        package
            .manifest()
            .get("license")
            .cloned()
            .unwrap_or(Value::Null),
    );
    report.insert(
        "source".to_owned(),
        package
            .manifest()
            .get("source")
            .cloned()
            .unwrap_or(Value::Null),
    );
    Ok(Value::Object(report))
}

fn capability_report(
    modules: &[ModuleData],
    manifest_requested: &[String],
) -> Result<Value, String> {
    let mut per_module = Vec::new();
    let mut merged_libtessera = BTreeSet::new();
    let mut merged_requires = BTreeSet::new();
    let mut merged_platform = BTreeSet::new();
    let mut merged_implied = BTreeSet::new();

    for module in modules {
        let mut libtessera = Vec::new();
        for reference in &module.capabilities.reached_libtessera_calls {
            let imported = resolve_import(module, reference.import_index, reference.entity_index)?;
            if imported.0.name != "LibTessera" && !imported.0.name.starts_with("LibTessera.") {
                return Err(format!(
                    "module '{}' CAPS LibTessera reference points to '{}', not LibTessera",
                    module.record.name, imported.0.name
                ));
            }
            let row = json!({
                "module": imported.0.name,
                "symbol": imported.1.name,
                "abiUuid": uuid(&imported.0.abi_uuid),
            });
            merged_libtessera.insert(canonical_key(&row));
            libtessera.push(row);
        }

        let mut requires = Vec::new();
        for reference in &module.capabilities.reached_requires_functions {
            let imported = resolve_import(module, reference.import_index, reference.entity_index)?;
            let row = json!({
                "module": imported.0.name,
                "symbol": imported.1.name,
                "abiUuid": uuid(&imported.0.abi_uuid),
            });
            merged_requires.insert(canonical_key(&row));
            requires.push(row);
        }

        let mut platform_imports = Vec::new();
        for reference in &module.capabilities.reached_platform_imports {
            let imported = resolve_import(module, reference.import_index, reference.entity_index)?;
            let row = json!({
                "library": imported.0.name,
                "symbol": imported.1.platform_symbol.as_deref().unwrap_or(&imported.1.name),
                "kind": import_entity_kind(imported.1.kind),
                "weak": imported.0.weak,
            });
            merged_platform.insert(canonical_key(&row));
            platform_imports.push(row);
        }

        merged_implied.extend(module.capabilities.implied_entitlements.iter().cloned());
        per_module.push(json!({
            "module": module.record.name,
            "libtesseraCalls": libtessera,
            "requires": requires,
            "platformImports": platform_imports,
            "impliedEntitlements": module.capabilities.implied_entitlements,
            "requestedEntitlements": module.capabilities.requested_entitlements,
        }));
    }
    per_module.sort_by(|left, right| left["module"].as_str().cmp(&right["module"].as_str()));

    Ok(json!({
        "modules": per_module,
        "merged": {
            "libtesseraCalls": values(merged_libtessera),
            "requires": values(merged_requires),
            "platformImports": values(merged_platform),
            "impliedEntitlements": strings(merged_implied),
            "requestedEntitlements": manifest_requested,
        }
    }))
}

fn resolve_import(
    module: &ModuleData,
    import_index: u64,
    entity_index: u64,
) -> Result<(&tir_format::BinaryImport, &tir_format::BinaryImportedEntity), String> {
    let import = module
        .imports
        .get(usize::try_from(import_index).map_err(|_| "CAPS import index overflows".to_owned())?)
        .ok_or_else(|| {
            format!(
                "module '{}' CAPS import index is out of range",
                module.record.name
            )
        })?;
    let entity = import
        .entities
        .get(usize::try_from(entity_index).map_err(|_| "CAPS entity index overflows".to_owned())?)
        .ok_or_else(|| {
            format!(
                "module '{}' CAPS entity index is out of range",
                module.record.name
            )
        })?;
    Ok((import, entity))
}

fn trust_report(modules: &[ModuleData]) -> Value {
    let mut groups: [Vec<Value>; 3] = std::array::from_fn(|_| Vec::new());
    for module in modules {
        for (index, function) in module.analysis.functions.iter().enumerate() {
            let Some(signature) = module.signatures.records.get(index) else {
                continue;
            };
            let mut row = json!({
                "module": module.record.name,
                "function": signature.name,
            });
            if function.trust_class == 2 {
                row["reasonMask"] = json!(function.reason_mask);
            }
            if let Some(group) = groups.get_mut(usize::from(function.trust_class)) {
                group.push(row);
            }
        }
    }
    for group in &mut groups {
        group.sort_by(|left, right| {
            (field(left, "module"), field(left, "function"))
                .cmp(&(field(right, "module"), field(right, "function")))
        });
    }
    json!({
        "managed": { "count": groups[0].len(), "functions": groups[0] },
        "sandboxed": { "count": groups[1].len(), "functions": groups[1] },
        "unmanaged": { "count": groups[2].len(), "functions": groups[2] },
    })
}

fn semantics_report(modules: &[ModuleData]) -> Value {
    let mut defined = 0_u64;
    let mut permissive = 0_u64;
    for module in modules {
        for signature in &module.signatures.records {
            let mode = signature.semantics.unwrap_or(module.record.semantics);
            match mode {
                tir_format::SemanticsMode::Defined => defined += 1,
                tir_format::SemanticsMode::Permissive => permissive += 1,
            }
        }
    }
    json!({ "defined": defined, "permissive": permissive })
}

fn assembly_report(modules: &[ModuleData]) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    for module in modules {
        for region in &module.analysis.assembly_regions {
            let function = module
                .signatures
                .records
                .get(
                    usize::try_from(region.function_index)
                        .map_err(|_| "assembly function index overflows".to_owned())?,
                )
                .ok_or_else(|| {
                    format!(
                        "module '{}' assembly function index is out of range",
                        module.record.name
                    )
                })?;
            output.push(json!({
                "module": module.record.name,
                "function": function.name,
                "instruction": region.instruction_ordinal,
                "architecture": architecture(region.architecture),
                "effects": region.effects,
            }));
        }
    }
    output.sort_by(|left, right| {
        (
            field(left, "module"),
            field(left, "function"),
            left["instruction"].as_u64().unwrap_or_default(),
        )
            .cmp(&(
                field(right, "module"),
                field(right, "function"),
                right["instruction"].as_u64().unwrap_or_default(),
            ))
    });
    Ok(output)
}

fn variant_report(modules: &[ModuleData]) -> Vec<Value> {
    let mut output = Vec::new();
    for module in modules {
        for variant_set in &module.variants {
            let members = variant_set
                .members
                .iter()
                .filter_map(|member| {
                    let function = module
                        .signatures
                        .records
                        .get(member.function_index as usize)?;
                    Some(json!({
                        "architecture": architecture(member.architecture),
                        "features": member.features,
                        "function": function.name,
                    }))
                })
                .collect::<Vec<_>>();
            output.push(json!({
                "module": module.record.name,
                "name": variant_set.name,
                "members": members,
            }));
        }
    }
    output.sort_by(|left, right| {
        (field(left, "module"), field(left, "name"))
            .cmp(&(field(right, "module"), field(right, "name")))
    });
    output
}

fn dynamic_code_report(modules: &[ModuleData], manifest_requested: &[String]) -> Value {
    let mut runtime_functions = Vec::new();
    let mut api_functions = BTreeSet::new();
    let mut raw_jit_entitlements = BTreeSet::new();
    for module in modules {
        for signature in &module.signatures.records {
            if signature.engine == Some(tir_format::BinaryEngine::Runtime) {
                runtime_functions
                    .push(json!({ "module": module.record.name, "function": signature.name }));
            }
        }
        for import in &module.imports {
            if import.kind == BinaryImportKind::Ridl && import.name == "TIRRuntime" {
                for entity in &import.entities {
                    if matches!(entity.name.as_str(), "load" | "lookup" | "unload") {
                        api_functions.insert(entity.name.clone());
                    }
                }
            }
        }
        for entitlement in manifest_requested {
            if entitlement.contains("jit") {
                raw_jit_entitlements.insert(entitlement.clone());
            }
        }
    }
    runtime_functions.sort_by(|left, right| {
        (field(left, "module"), field(left, "function"))
            .cmp(&(field(right, "module"), field(right, "function")))
    });
    json!({
        "usesApi": !api_functions.is_empty(),
        "apiFunctions": api_functions.into_iter().collect::<Vec<_>>(),
        "rawJitEntitlements": raw_jit_entitlements.into_iter().collect::<Vec<_>>(),
        "runtimeFunctions": runtime_functions,
    })
}

fn canonical_key(value: &Value) -> String {
    crate::tirc::jcs::canonical_json(value)
        .expect("report sort keys contain RFC 8785-compatible JSON")
}

pub(in crate::tirc) fn canonical_json(value: &Value) -> Result<String, String> {
    let mut value = value.clone();
    replace_large_report_integers(&mut value);
    crate::tirc::jcs::canonical_json(&value)
}

fn replace_large_report_integers(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(integer) = number
                .as_u64()
                .filter(|integer| *integer > crate::tirc::jcs::MAX_SAFE_INTEGER)
            {
                *value = Value::String(integer.to_string());
            }
        }
        Value::Array(values) => {
            for value in values {
                replace_large_report_integers(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                replace_large_report_integers(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

fn values(values: BTreeSet<String>) -> Vec<Value> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_str(&value).ok())
        .collect()
}

fn strings(values: BTreeSet<String>) -> Vec<String> {
    values.into_iter().collect()
}

pub(super) fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

pub(super) fn uuid(bytes: &[u8; 16]) -> String {
    let hex = |slice: &[u8]| {
        slice
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..])
    )
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn architecture(code: u8) -> &'static str {
    match code {
        1 => "aarch64",
        2 => "x64",
        3 => "rv64",
        4 => "wasm",
        _ => "portable",
    }
}

pub(super) fn import_entity_kind(kind: BinaryImportEntityKind) -> &'static str {
    match kind {
        BinaryImportEntityKind::Function => "function",
        BinaryImportEntityKind::Global => "global",
        BinaryImportEntityKind::Type => "type",
        BinaryImportEntityKind::Witness => "witness",
        BinaryImportEntityKind::Descriptor => "descriptor",
        BinaryImportEntityKind::PlatformData => "data",
    }
}
