use std::path::Path;
use std::process::ExitCode;

use tir_diag::Severity;
use tir_format::{
    BinaryAnalysisSummary, BinaryCapabilities, BinaryConstantTable, BinaryGpuTable, BinaryImport,
    BinaryModule, BinaryModuleRecord, BinaryRidlTable, BinarySignatureTable, BinaryVariantSet,
};

use super::package::PackageReader;

pub(in crate::tirc) mod report;

pub(crate) struct ModuleData {
    pub(crate) module: BinaryModule,
    pub(crate) record: BinaryModuleRecord,
    pub(crate) imports: Vec<BinaryImport>,
    pub(crate) constants: BinaryConstantTable,
    pub(crate) signatures: BinarySignatureTable,
    pub(crate) analysis: BinaryAnalysisSummary,
    pub(crate) variants: Vec<BinaryVariantSet>,
    pub(crate) gpu: BinaryGpuTable,
    pub(crate) capabilities: BinaryCapabilities,
    pub(crate) ridl: BinaryRidlTable,
    pub(crate) tir_version: String,
}

pub(super) fn command(path: &str) -> ExitCode {
    let package = match PackageReader::open(Path::new(path)) {
        Ok(package) => package,
        Err(error) => return super::fail(&error),
    };
    if let Err(error) = package.verify_archive_entries() {
        return super::fail(&error);
    }
    let manifest = package.manifest();
    if let Err(error) = super::package::validate_manifest(manifest) {
        return super::fail(&format!("tirc analyze: {error}"));
    }
    let manifest_bytes = match package.read("manifest.json") {
        Ok(bytes) => bytes,
        Err(error) => return super::fail(&error),
    };
    let canonical_manifest = match super::jcs::canonical_json(manifest) {
        Ok(manifest) => manifest,
        Err(error) => return super::fail(&format!("tirc analyze: {error}")),
    };
    if manifest_bytes != canonical_manifest.as_bytes() {
        return super::fail("tirc analyze: manifest.json is not canonical JCS");
    }
    let digest = *blake3::hash(&manifest_bytes).as_bytes();
    let signature = match package.read("signatures/developer.sig") {
        Ok(signature) => signature,
        Err(error) => return super::fail(&error),
    };
    if let Err(error) = super::package::verify_package_signature(&signature, &digest) {
        return super::fail(&format!("tirc analyze: {error}"));
    }
    let Some(package_profile) = manifest.get("profile").and_then(|value| value.as_str()) else {
        return super::fail("tirc analyze: manifest profile is missing or not a string");
    };
    let Some(tir_version) = manifest.get("tir").and_then(|value| value.as_str()) else {
        return super::fail("tirc analyze: manifest tir version is missing or not a string");
    };
    let Some(module_records) = manifest.get("modules").and_then(|value| value.as_array()) else {
        return super::fail("tirc analyze: manifest modules must be an array");
    };
    let platforms = match super::package::manifest_platforms(manifest) {
        Ok(platforms) => platforms,
        Err(error) => return super::fail(&format!("tirc analyze: {error}")),
    };
    let source_artifact = if package_profile == "driver" {
        let (canonical, hash) = match super::package::driver_source_artifact(manifest) {
            Ok(source) => source,
            Err(error) => return super::fail(&format!("tirc analyze: {error}")),
        };
        let source_json = match package.read("source.json") {
            Ok(source) => source,
            Err(error) => return super::fail(&error),
        };
        if source_json != canonical {
            return super::fail(
                "tirc analyze: source.json does not match the canonical manifest source",
            );
        }
        Some(hash)
    } else {
        None
    };

    if let Err(error) = verify_declared_files(&package) {
        return super::fail(&error);
    }
    let mut modules = Vec::with_capacity(module_records.len());
    let mut binaries = Vec::with_capacity(module_records.len());
    let mut module_names = std::collections::BTreeSet::new();
    for entry in module_records {
        let Some(module_name) = entry.get("name").and_then(|value| value.as_str()) else {
            return super::fail("tirc analyze: each manifest module needs a string name");
        };
        if module_name.is_empty() || !module_names.insert(module_name) {
            return super::fail("tirc analyze: manifest module names must be nonempty and unique");
        }
        let Some(module_path) = entry.get("path").and_then(|value| value.as_str()) else {
            return super::fail("tirc analyze: each manifest module needs a string path");
        };
        let bytes = match package.read(module_path) {
            Ok(bytes) => bytes,
            Err(error) => return super::fail(&error),
        };
        let module = match BinaryModule::decode(&bytes) {
            Ok(module) => module,
            Err(error) => {
                super::render_binary_diagnostic(module_path, &error.diagnostic);
                return ExitCode::FAILURE;
            }
        };
        let mut canonical = Vec::new();
        if let Err(error) = module.write_uncompressed_container(&mut canonical) {
            return super::fail(&format!(
                "tirc analyze: cannot re-encode package module {module_name}: {error}"
            ));
        }
        if canonical != bytes {
            return super::fail(&format!(
                "tirc analyze: package module {module_name} is not canonical (TIR-ENC-0002)"
            ));
        }
        let mut invalid = false;
        for diagnostic in tir_verify::verify_binary_module_records(&module, u64::MAX) {
            invalid |= diagnostic.severity == Severity::Error;
            super::render_binary_diagnostic(module_path, &diagnostic);
        }
        if invalid {
            return ExitCode::FAILURE;
        }
        let data = match decode_module(module_path, module) {
            Ok(data) => data,
            Err(error) => return super::fail(&error),
        };
        if data.record.name != module_name {
            return super::fail(&format!(
                "tirc analyze: manifest module `{module_name}` names `{}`",
                data.record.name
            ));
        }
        if data.module.header.profile.name() != package_profile {
            return super::fail(&format!(
                "tirc analyze: module `{module_name}` profile does not match the manifest"
            ));
        }
        let platform = data.module.header.platform.name();
        if platform != "portable" && !platforms.iter().any(|listed| listed == platform) {
            return super::fail(&format!(
                "tirc analyze: module {module_name} targets unlisted platform {platform}"
            ));
        }
        if data.tir_version != tir_version {
            return super::fail(&format!(
                "tirc analyze: module `{module_name}` TIR version does not match the manifest"
            ));
        }
        if source_artifact.is_some_and(|hash| data.record.source_hash != Some(hash)) {
            return super::fail(&format!(
                "tirc analyze: module `{module_name}` source hash does not match source.json"
            ));
        }
        binaries.push(data.module.clone());
        modules.push(data);
    }
    if let Err(error) = super::package::verify_entry(manifest, &modules) {
        return super::fail(&format!("tirc analyze: {error}"));
    }
    let entitlements = match manifest.get("entitlements") {
        None => Vec::new(),
        Some(value) => {
            let Some(items) = value.as_array() else {
                return super::fail("tirc analyze: manifest entitlements must be an array");
            };
            let mut entitlements = Vec::with_capacity(items.len());
            for item in items {
                let Some(entitlement) = item.as_str() else {
                    return super::fail(
                        "tirc analyze: manifest entitlements must contain only strings",
                    );
                };
                entitlements.push(entitlement.to_owned());
            }
            entitlements
        }
    };
    let diagnostics = tir_verify::verify_binary_package_capabilities(&binaries, &entitlements);
    let mut invalid = false;
    for diagnostic in diagnostics {
        invalid |= diagnostic.severity == Severity::Error;
        super::render_binary_diagnostic(path, &diagnostic);
    }
    if invalid {
        return ExitCode::FAILURE;
    }
    let report = match report::create(&package, &modules) {
        Ok(report) => report,
        Err(error) => return super::fail(&error),
    };
    let canonical_report = match report::canonical_json(&report) {
        Ok(report) => report,
        Err(error) => return super::fail(&format!("tirc analyze: {error}")),
    };
    println!("{canonical_report}");
    ExitCode::SUCCESS
}

fn verify_declared_files(package: &PackageReader) -> Result<(), String> {
    let mut paths = std::collections::BTreeSet::new();
    for field in ["modules", "resources", "profiles"] {
        let Some(entries) = package.manifest().get(field) else {
            continue;
        };
        let entries = entries
            .as_array()
            .ok_or_else(|| format!("manifest `{field}` must be an array"))?;
        for entry in entries {
            let path = entry
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("manifest `{field}` entry is missing its path"))?;
            if !paths.insert(path) {
                return Err(format!("manifest repeats package file path '{path}'"));
            }
            let expected = entry
                .get("blake3")
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("manifest `{field}` entry is missing its BLAKE3 hash"))?;
            let (actual, actual_size) = package.digest(path)?;
            if hex(&actual) != expected {
                return Err(format!(
                    "package file '{path}' does not match its BLAKE3 hash"
                ));
            }
            if field == "resources" || entry.get("size").is_some() {
                let declared_size = entry
                    .get("size")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| format!("manifest {field} entry has an invalid size"))?;
                if declared_size != actual_size {
                    return Err(format!(
                        "package file '{path}' does not match its declared size"
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(in crate::tirc) fn decode_module(
    path: &str,
    module: BinaryModule,
) -> Result<ModuleData, String> {
    let memory_limit = u64::MAX;
    let record = module
        .module_record(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let imports = module
        .imports(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let types = module
        .type_table(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let constants = module
        .constant_table(memory_limit, &types)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let signatures = module
        .signature_table(memory_limit, &types)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let analysis = module
        .analysis_summary(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let variants = module
        .variant_sets(memory_limit, &signatures)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let gpu = module
        .gpu_table(memory_limit, &signatures, &types, &constants)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let capabilities = module
        .capabilities(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let ridl = module
        .ridl_table(memory_limit)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let tir_version = format!(
        "{}.{}.{}",
        module.header.year, module.header.month, module.header.week
    );
    if !module.header.claims_canonical() {
        return Err(format!(
            "{path}: package module does not claim canonical encoding"
        ));
    }
    Ok(ModuleData {
        module,
        record,
        imports,
        constants,
        signatures,
        analysis,
        variants,
        gpu,
        capabilities,
        ridl,
        tir_version,
    })
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
