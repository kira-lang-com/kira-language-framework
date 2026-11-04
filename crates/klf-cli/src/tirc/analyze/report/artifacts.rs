use std::collections::BTreeMap;

use serde_json::{Value, json};
use tir_format::{BinaryConstantValue, BinaryEntryKind, BinaryImportKind, BinaryRidlExportKind};

use super::super::ModuleData;
use super::{field, hex, import_entity_kind, uuid};

const GPU_CAPABILITIES: [&str; 19] = [
    "float64",
    "atomic128",
    "float16.atomics",
    "tile",
    "framebuffer_fetch",
    "raster_order",
    "work_graphs",
    "work_graphs.mesh",
    "coop.fp8",
    "coop.blockscaled",
    "coop.reduce",
    "compute_derivatives",
    "float32.denorm_preserve",
    "sparse_residency",
    "sampler_feedback",
    "subgroup_size_control",
    "triangle_vertex_positions",
    "shading_rate",
    "printf",
];

pub(super) fn gpu_report(modules: &[ModuleData]) -> Vec<Value> {
    let mut entries = Vec::new();
    for module in modules {
        for entry in &module.gpu.entries {
            let Some(signature) = module.signatures.records.get(entry.function_index as usize)
            else {
                continue;
            };
            let capabilities = GPU_CAPABILITIES
                .iter()
                .enumerate()
                .filter_map(|(bit, name)| (entry.capabilities & (1 << bit) != 0).then_some(*name))
                .collect::<Vec<_>>();
            entries.push(json!({
                "module": module.record.name,
                "function": signature.name,
                "kind": entry_kind(entry.kind),
                "threads": entry.threads,
                "capabilities": capabilities,
            }));
        }
    }
    entries.sort_by(|left, right| {
        (field(left, "module"), field(left, "function"))
            .cmp(&(field(right, "module"), field(right, "function")))
    });
    entries
}

pub(super) fn ridl_report(modules: &[ModuleData]) -> Vec<Value> {
    let mut output = Vec::new();
    for module in modules {
        let exports = module
            .ridl
            .exports
            .iter()
            .map(|export| {
                json!({
                    "name": export.symbol,
                    "kind": ridl_export_kind(export.kind),
                    "entityIndex": export.entity_index,
                    "abiTag": hex(&export.abi_uuid[..8]),
                    "abiUuid": uuid(&export.abi_uuid),
                })
            })
            .collect::<Vec<_>>();

        let mut imports = module
            .imports
            .iter()
            .filter(|import| import.kind == BinaryImportKind::Ridl)
            .map(|import| {
                let mut entities = import
                    .entities
                    .iter()
                    .map(|entity| {
                        json!({ "name": entity.name, "kind": import_entity_kind(entity.kind) })
                    })
                    .collect::<Vec<_>>();
                entities.sort_by(|left, right| {
                    (field(left, "kind"), field(left, "name"))
                        .cmp(&(field(right, "kind"), field(right, "name")))
                });
                json!({
                    "module": import.name,
                    "abiUuid": uuid(&import.abi_uuid),
                    "entities": entities,
                })
            })
            .collect::<Vec<_>>();
        imports.sort_by(|left, right| field(left, "module").cmp(field(right, "module")));

        let stamps = module
            .ridl
            .abi_stamps
            .iter()
            .map(|stamp| json!({ "symbol": stamp.symbol, "abiUuid": uuid(&stamp.uuid) }))
            .collect::<Vec<_>>();
        let inlinable_bodies = module
            .ridl
            .inlinable_bodies
            .iter()
            .filter_map(|body| {
                let function = module
                    .signatures
                    .records
                    .get(usize::try_from(body.function_index).ok()?)?;
                Some(json!({ "function": function.name, "blake3": hex(&body.hash) }))
            })
            .collect::<Vec<_>>();
        output.push(json!({
            "module": module.record.name,
            "exports": exports,
            "imports": imports,
            "abiStamps": stamps,
            "canonicalHash": module.ridl.canonical_hash.map(|hash| hex(&hash)),
            "inlinableBodies": inlinable_bodies,
        }));
    }
    output.sort_by(|left, right| field(left, "module").cmp(field(right, "module")));
    output
}

pub(super) fn string_report(modules: &[ModuleData]) -> Vec<Value> {
    let mut values = BTreeMap::new();
    for module in modules {
        for constant in &module.constants.records {
            collect_strings(&constant.value, &mut |value| {
                if let Some(kind) = string_kind(value) {
                    values
                        .entry((module.record.name.clone(), value.to_owned()))
                        .or_insert(kind);
                }
            });
        }
    }
    values
        .into_iter()
        .map(|((module, value), kind)| json!({ "module": module, "value": value, "kind": kind }))
        .collect()
}

fn collect_strings(value: &BinaryConstantValue, visit: &mut impl FnMut(&str)) {
    match value {
        BinaryConstantValue::String(value) => visit(value),
        BinaryConstantValue::Tuple(values)
        | BinaryConstantValue::Array(values)
        | BinaryConstantValue::Vector(values)
        | BinaryConstantValue::Struct(values)
        | BinaryConstantValue::Enum {
            payload: values, ..
        } => {
            for value in values {
                collect_strings(value, visit);
            }
        }
        BinaryConstantValue::Union { value, .. } => collect_strings(value, visit),
        BinaryConstantValue::Integer(_)
        | BinaryConstantValue::Float(_)
        | BinaryConstantValue::Zero
        | BinaryConstantValue::Bytes(_)
        | BinaryConstantValue::Null
        | BinaryConstantValue::Sampler(_) => {}
    }
}

fn string_kind(value: &str) -> Option<&'static str> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return valid_host(value).then_some("host");
    };
    let mut scheme_bytes = scheme.bytes();
    if !scheme_bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        || !scheme_bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte))
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .trim();
    (!host_port.is_empty() && valid_url_host_port(host_port)).then_some("url")
}

fn hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || !value.is_ascii() {
        return false;
    }
    let value = value.strip_suffix('.').unwrap_or(value);
    !value.is_empty()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        })
}

fn valid_host(value: &str) -> bool {
    hostname(value) || value.parse::<std::net::Ipv4Addr>().is_ok()
}

fn valid_url_host_port(value: &str) -> bool {
    if let Some(bracketed) = value.strip_prefix('[') {
        let Some((address, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        if address.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        return valid_port_suffix(suffix);
    }
    let mut parts = value.split(':');
    let host = parts.next().unwrap_or_default();
    let Some(port) = parts.next() else {
        return valid_host(host);
    };
    parts.next().is_none() && valid_host(host) && valid_port_suffix(&format!(":{port}"))
}

fn valid_port_suffix(suffix: &str) -> bool {
    if suffix.is_empty() {
        return true;
    }
    suffix
        .strip_prefix(':')
        .and_then(|port| port.parse::<u16>().ok())
        .is_some()
}

fn entry_kind(kind: BinaryEntryKind) -> &'static str {
    match kind {
        BinaryEntryKind::Compute => "compute",
        BinaryEntryKind::Object => "object",
        BinaryEntryKind::Mesh => "mesh",
        BinaryEntryKind::Vertex => "vertex",
        BinaryEntryKind::Fragment => "fragment",
        BinaryEntryKind::Tile => "tile",
        BinaryEntryKind::Raygen => "raygen",
        BinaryEntryKind::Intersection => "intersection",
        BinaryEntryKind::Anyhit => "anyhit",
        BinaryEntryKind::Closesthit => "closesthit",
        BinaryEntryKind::Miss => "miss",
        BinaryEntryKind::Callable => "callable",
        BinaryEntryKind::Node => "node",
    }
}

fn ridl_export_kind(kind: BinaryRidlExportKind) -> &'static str {
    match kind {
        BinaryRidlExportKind::Function => "function",
        BinaryRidlExportKind::Method => "method",
        BinaryRidlExportKind::Descriptor => "descriptor",
        BinaryRidlExportKind::Builder => "builder",
        BinaryRidlExportKind::DescriptorSize => "descriptorSize",
        BinaryRidlExportKind::Witness => "witness",
        BinaryRidlExportKind::Instance => "instance",
        BinaryRidlExportKind::Global => "global",
    }
}
