use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Map, Value};
use tir_diag::Severity;
use tir_format::{BinaryModule, Section, StringTable};

use super::PackageReader;
use super::archive::{ArchiveContent, write_archive};
use super::sign::sign_digest;

struct Options {
    directory: PathBuf,
    manifest: PathBuf,
    identity: PathBuf,
    output: PathBuf,
}

pub(in crate::tirc) fn command(arguments: &[String]) -> ExitCode {
    let options = match parse_options(arguments) {
        Ok(options) => options,
        Err(error) => return fail(&error),
    };
    match package(&options) {
        Ok(()) => {
            println!("Packaged: {}", options.output.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

fn package(options: &Options) -> Result<(), String> {
    let manifest_bytes = fs::read(&options.manifest).map_err(|error| {
        format!(
            "cannot read manifest '{}': {error}",
            options.manifest.display()
        )
    })?;
    let mut manifest = crate::tirc::jcs::parse_json(&manifest_bytes, "manifest")?;
    validate_manifest(&manifest)?;
    let mut entitlements = read_string_array(&manifest, "entitlements")?;
    entitlements.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    if entitlements.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("manifest entitlements must be unique".to_owned());
    }
    if let Some(object) = manifest.as_object_mut() {
        object.insert("entitlements".to_owned(), serde_json::json!(entitlements));
    }
    let platforms = super::manifest_platforms(&manifest)?;
    let profile = required_string(&manifest, "profile", "manifest")?.to_owned();
    let tir_version = required_string(&manifest, "tir", "manifest")?.to_owned();
    let source_artifact = if profile == "driver" {
        Some(super::driver_source_artifact(&manifest)?)
    } else {
        None
    };

    let package = PackageReader::with_manifest(&options.directory, manifest.clone())?;
    let mut contents = BTreeMap::new();
    let mut declared_paths = [
        "analysis.json".to_owned(),
        "manifest.json".to_owned(),
        "signatures/developer.sig".to_owned(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if manifest.get("profile").and_then(Value::as_str) == Some("driver") {
        declared_paths.insert("source.json".to_owned());
    }
    let module_records = manifest
        .get_mut("modules")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "manifest modules must be an array".to_owned())?;
    let mut modules = Vec::with_capacity(module_records.len());
    let mut binaries = Vec::with_capacity(module_records.len());
    let mut module_names = BTreeSet::new();

    for record in module_records {
        let name = required_string(record, "name", "module")?.to_owned();
        if name.is_empty() || !module_names.insert(name.clone()) {
            return Err("manifest module names must be nonempty and unique".to_owned());
        }
        let path = required_string(record, "path", "module")?.to_owned();
        add_declared_path(&mut declared_paths, &path)?;
        let input = package.read(&path)?;
        let bytes = canonicalize_module(&path, &input, &entitlements)?;
        update_file_record(record, &bytes, false)?;
        let module = BinaryModule::decode(&bytes)
            .map_err(|error| format!("{path}: {}", error.diagnostic))?;
        for diagnostic in tir_verify::verify_binary_module_records(&module, u64::MAX) {
            if diagnostic.severity == Severity::Error {
                return Err(format!("{path}: {diagnostic}"));
            }
        }
        let data = crate::tirc::analyze::decode_module(&path, module)
            .map_err(|error| error.to_string())?;
        if data.record.name != name {
            return Err(format!("manifest module {name} names {}", data.record.name));
        }
        if data.module.header.profile.name() != profile {
            return Err(format!("module {name} profile does not match the package"));
        }
        if data.tir_version != tir_version {
            return Err(format!(
                "module {name} TIR version does not match the package"
            ));
        }
        if source_artifact
            .as_ref()
            .is_some_and(|(_, hash)| data.record.source_hash != Some(*hash))
        {
            return Err(format!(
                "module {name} source hash does not match canonical source.json"
            ));
        }
        let module_platform = data.module.header.platform.name();
        if module_platform != "portable"
            && !platforms.iter().any(|platform| platform == module_platform)
        {
            return Err(format!(
                "module {name} targets unlisted platform {module_platform}"
            ));
        }
        contents.insert(path, ArchiveContent::Bytes(bytes));
        binaries.push(data.module.clone());
        modules.push(data);
    }

    for field in ["resources", "profiles"] {
        if manifest.get(field).is_none() {
            continue;
        }
        let records = manifest
            .get_mut(field)
            .and_then(Value::as_array_mut)
            .ok_or_else(|| format!("manifest {field} must be an array"))?;
        for record in records {
            let path = required_string(record, "path", field)?.to_owned();
            add_declared_path(&mut declared_paths, &path)?;
            let (hash, size) = package.digest(&path)?;
            update_digest_record(record, &hash, size, field == "resources")?;
            contents.insert(
                path.clone(),
                ArchiveContent::File(package.source_path(&path)?),
            );
        }
    }

    tir_verify::verify_binary_package_capabilities(&binaries, &entitlements)
        .into_iter()
        .try_for_each(|diagnostic| {
            if diagnostic.severity == Severity::Error {
                Err(diagnostic.to_string())
            } else {
                Ok(())
            }
        })?;
    verify_entry(&manifest, &modules)?;

    let report_package = PackageReader::with_manifest(&options.directory, manifest.clone())?;
    let report = crate::tirc::analyze::report::create(&report_package, &modules)?;
    contents.insert(
        "analysis.json".to_owned(),
        ArchiveContent::Bytes(crate::tirc::analyze::report::canonical_json(&report)?.into_bytes()),
    );
    if let Some((source_bytes, _)) = source_artifact {
        contents.insert(
            "source.json".to_owned(),
            ArchiveContent::Bytes(source_bytes),
        );
    }

    let manifest_bytes = crate::tirc::jcs::canonical_json(&manifest)?.into_bytes();
    let digest = *blake3::hash(&manifest_bytes).as_bytes();
    let signature = sign_digest(&options.identity, &digest)?;
    contents.insert(
        "manifest.json".to_owned(),
        ArchiveContent::Bytes(manifest_bytes),
    );
    contents.insert(
        "signatures/developer.sig".to_owned(),
        ArchiveContent::Bytes(signature),
    );
    write_archive(&options.output, &contents)
}

fn canonicalize_module(
    path: &str,
    input: &[u8],
    entitlements: &[String],
) -> Result<Vec<u8>, String> {
    let mut module =
        BinaryModule::decode(input).map_err(|error| format!("{path}: {}", error.diagnostic))?;
    if !module.header.claims_canonical() {
        return Err(format!(
            "{path}: tirc package currently requires a module with the canonical flag"
        ));
    }
    let mut capabilities = tir_verify::recompute_binary_module_capabilities(&module, entitlements)
        .map_err(|error| format!("{path}: {error}"))?;
    capabilities.reached_requires_functions.sort_unstable();
    capabilities.reached_requires_functions.dedup();
    capabilities
        .implied_entitlements
        .sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    capabilities.implied_entitlements.dedup();

    let original_strings = module
        .string_table(u64::MAX)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let mut values = (0..original_strings.len())
        .filter_map(|index| original_strings.get(index).map(str::to_owned))
        .collect::<Vec<_>>();
    for value in capabilities
        .implied_entitlements
        .iter()
        .chain(&capabilities.requested_entitlements)
    {
        if !values.iter().any(|present| present == value) {
            values.push(value.clone());
        }
    }
    let strings = StringTable::from_values(values)
        .map_err(|error| format!("{path}: invalid STRINGS table: {}", error.message))?;
    let mut string_bytes = Vec::new();
    strings
        .encode(&mut string_bytes)
        .map_err(|error| format!("{path}: cannot encode STRINGS: {error}"))?;
    let imports = module
        .imports(u64::MAX)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;
    let capability_bytes = capabilities
        .encode(&strings, &imports)
        .map_err(|error| format!("{path}: {}", error.diagnostic))?;

    let mut has_strings = false;
    let mut has_capabilities = false;
    for section in &mut module.sections {
        match section.kind {
            1 => {
                *section = Section::from_uncompressed(1, string_bytes.clone());
                has_strings = true;
            }
            13 => {
                *section = Section::from_uncompressed(13, capability_bytes.clone());
                has_capabilities = true;
            }
            _ => {}
        }
    }
    if !has_strings {
        return Err(format!("{path}: STRINGS section is missing"));
    }
    if !has_capabilities {
        module
            .sections
            .push(Section::from_uncompressed(13, capability_bytes));
    }
    module.sections.retain(|section| section.kind != 255);
    let mut output = Vec::new();
    module
        .write_uncompressed_container(&mut output)
        .map_err(|error| format!("{path}: cannot re-encode TIR module: {}", error.diagnostic))?;
    Ok(output)
}

pub(in crate::tirc) fn validate_manifest(manifest: &Value) -> Result<(), String> {
    if manifest.get("format").and_then(Value::as_str) != Some("tirpkg") {
        return Err("manifest format must be tirpkg".to_owned());
    }
    for field in ["tir", "id", "name", "version", "kind", "profile"] {
        required_string(manifest, field, "manifest")?;
    }
    let id = required_string(manifest, "id", "manifest")?;
    if !valid_package_id(id) {
        return Err("manifest id must be a reverse-DNS package identifier".to_owned());
    }
    for field in ["name", "version"] {
        if required_string(manifest, field, "manifest")?.is_empty() {
            return Err(format!("manifest {field} must not be empty"));
        }
    }
    if manifest.get("build").and_then(Value::as_u64).is_none() {
        return Err("manifest build must be an unsigned integer".to_owned());
    }
    let kind = required_string(manifest, "kind", "manifest")?;
    let profile = required_string(manifest, "profile", "manifest")?;
    let compatible = match kind {
        "app" | "extension" => profile == "app",
        "tool" | "framework" => matches!(profile, "app" | "system"),
        "service" | "runtime" => profile == "system",
        "driver" => profile == "driver",
        "kernel" => profile == "kernel",
        _ => false,
    };
    if !compatible {
        return Err("manifest kind and profile are incompatible".to_owned());
    }
    let platforms = super::manifest_platforms(manifest)?;
    if matches!(profile, "driver" | "kernel")
        && platforms.iter().any(|platform| platform != "tessera")
    {
        return Err("driver and kernel packages may list only tessera".to_owned());
    }
    if profile == "driver" {
        if required_string(manifest, "deviceKit", "driver manifest")?.is_empty() {
            return Err("driver manifest deviceKit must not be empty".to_owned());
        }
        if !matches!(
            manifest.get("license").and_then(Value::as_str),
            Some("GPL-3.0-only" | "GPL-3.0-or-later")
        ) {
            return Err(
                "driver manifest license must be GPL-3.0-only or GPL-3.0-or-later".to_owned(),
            );
        }
        super::driver_source_artifact(manifest)?;
    }
    let modules = manifest
        .get("modules")
        .and_then(Value::as_array)
        .ok_or_else(|| "manifest modules must be an array".to_owned())?;
    if modules.is_empty() {
        return Err("manifest must contain at least one module".to_owned());
    }
    for module in modules {
        required_string(module, "name", "module")?;
        required_string(module, "path", "module")?;
    }
    if let Some(dependencies) = manifest.get("dependencies") {
        let dependencies = dependencies
            .as_array()
            .ok_or_else(|| "manifest dependencies must be an array".to_owned())?;
        let mut names = BTreeSet::new();
        for dependency in dependencies {
            let module = required_string(dependency, "module", "dependency")?;
            if module.is_empty() || !names.insert(module) {
                return Err("manifest dependency modules must be nonempty and unique".to_owned());
            }
            let abi = required_string(dependency, "abi", "dependency")?;
            if !valid_uuid(abi) {
                return Err(format!("dependency `{module}` has an invalid ABI UUID"));
            }
        }
    }
    if let Some(resources) = manifest.get("resources") {
        let resources = resources
            .as_array()
            .ok_or_else(|| "manifest resources must be an array".to_owned())?;
        for resource in resources {
            let kind = match resource.get("kind") {
                None => "asset",
                Some(value) => value
                    .as_str()
                    .ok_or_else(|| "resource kind must be a string".to_owned())?,
            };
            if !matches!(kind, "asset" | "firmware") {
                return Err(format!("unsupported resource kind `{kind}`"));
            }
        }
    }
    Ok(())
}

fn valid_package_id(id: &str) -> bool {
    let labels = id.split('.').collect::<Vec<_>>();
    id.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    for (index, byte) in value.bytes().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte) {
            return false;
        }
    }
    true
}

fn read_string_array(manifest: &Value, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = manifest.get(field) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| format!("manifest {field} must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("manifest {field} must contain only strings"))
        })
        .collect()
}

fn required_string<'a>(value: &'a Value, field: &str, parent: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{parent} is missing string {field}"))
}

fn add_declared_path(paths: &mut BTreeSet<String>, path: &str) -> Result<(), String> {
    super::validate_package_path(path)?;
    if !paths.insert(path.to_owned()) {
        return Err(format!("manifest repeats package file path '{path}'"));
    }
    Ok(())
}

fn update_file_record(record: &mut Value, bytes: &[u8], with_size: bool) -> Result<(), String> {
    let hash = *blake3::hash(bytes).as_bytes();
    update_digest_record(record, &hash, bytes.len() as u64, with_size)
}

fn update_digest_record(
    record: &mut Value,
    hash: &[u8; 32],
    size: u64,
    with_size: bool,
) -> Result<(), String> {
    let object: &mut Map<String, Value> = record
        .as_object_mut()
        .ok_or_else(|| "manifest file record must be an object".to_owned())?;
    object.insert("blake3".to_owned(), Value::String(hex(hash)));
    if with_size || object.contains_key("size") {
        object.insert("size".to_owned(), Value::Number(size.into()));
    }
    Ok(())
}

pub(in crate::tirc) fn verify_entry(
    manifest: &Value,
    modules: &[crate::tirc::analyze::ModuleData],
) -> Result<(), String> {
    let kind = manifest
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let has_entry = matches!(
        kind,
        "app" | "extension" | "tool" | "service" | "driver" | "kernel"
    );
    let Some(entry) = manifest.get("entry") else {
        return if has_entry {
            Err(format!("package kind {kind} requires an entry"))
        } else {
            Ok(())
        };
    };
    if !has_entry {
        return Err(format!("package kind {kind} cannot declare an entry"));
    }
    let module_name = required_string(entry, "module", "manifest entry")?;
    let symbol = required_string(entry, "symbol", "manifest entry")?;
    let module = modules
        .iter()
        .find(|module| module.record.name == module_name)
        .ok_or_else(|| format!("entry names unknown module {module_name}"))?;
    if !module
        .signatures
        .records
        .iter()
        .any(|record| record.name == symbol && record.entry_kind.is_some())
    {
        return Err(format!(
            "entry symbol {module_name}::{symbol} is not a declared entry"
        ));
    }
    Ok(())
}

fn parse_options(arguments: &[String]) -> Result<Options, String> {
    let mut directory = None;
    let mut manifest = None;
    let mut identity = None;
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--manifest" => set_option(&mut manifest, arguments, &mut index, "--manifest")?,
            "--sign" => set_option(&mut identity, arguments, &mut index, "--sign")?,
            "-o" => set_option(&mut output, arguments, &mut index, "-o")?,
            value if value.starts_with('-') => {
                return Err(format!("unknown tirc package option '{value}'"));
            }
            value if directory.is_none() => directory = Some(PathBuf::from(value)),
            value => return Err(format!("unexpected tirc package argument '{value}'")),
        }
        index += 1;
    }
    Ok(Options {
        directory: directory.ok_or_else(|| {
            "usage: tirc package <dir> --manifest <file> --sign <identity> -o <pkg.tirpkg>"
                .to_owned()
        })?,
        manifest: manifest.ok_or_else(|| "tirc package: --manifest is required".to_owned())?,
        identity: identity.ok_or_else(|| "tirc package: --sign is required".to_owned())?,
        output: output.ok_or_else(|| "tirc package: -o is required".to_owned())?,
    })
}

fn set_option(
    slot: &mut Option<PathBuf>,
    arguments: &[String],
    index: &mut usize,
    flag: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("tirc package: repeated {flag}"));
    }
    let value = arguments
        .get(*index + 1)
        .ok_or_else(|| format!("tirc package: {flag} requires a value"))?;
    if value.starts_with('-') {
        return Err(format!("tirc package: {flag} requires a value"));
    }
    *slot = Some(PathBuf::from(value));
    *index += 1;
    Ok(())
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

fn fail(message: &str) -> ExitCode {
    eprintln!("tirc package: {message}");
    ExitCode::FAILURE
}
