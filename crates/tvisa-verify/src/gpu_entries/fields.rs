mod common;
mod special;
mod verify;

use std::collections::{HashMap, HashSet};
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_types::Type;
use tvisa_format::{
    TextGpuEntry, TextGpuEntryField as Field, TextGpuEntryValue as Value,
    TextGpuEntryValueKind as ValueKind,
};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub(super) fn verify(entry: &TextGpuEntry, stage: &str, base: usize) -> Vec<Diagnostic> {
    if let Some(error) = &entry.error {
        return vec![common::diagnostic(
            0,
            error.span,
            format!("invalid GPU entry record: {}", error.message),
        )];
    }
    verify::fields(&entry.fields, stage, base)
}

pub(super) fn declared_capabilities(entry: &TextGpuEntry) -> Option<HashSet<String>> {
    let value = field_value(entry, "capabilities")?;
    common::array(value)?
        .iter()
        .map(|value| common::scalar(value).map(str::to_owned))
        .collect()
}

pub(super) fn record_capability_requirements(entry: &TextGpuEntry, stage: &str) -> HashSet<String> {
    let mut required = HashSet::new();
    if stage == "tile" {
        required.insert("tile".to_owned());
    }
    if stage == "node" {
        required.insert("work_graphs".to_owned());
    }
    let value = |name: &str| field_value(entry, name);
    if value("subgroup_size")
        .and_then(common::scalar)
        .is_some_and(|size| size != "any")
    {
        required.insert("subgroup_size_control".to_owned());
    }
    if value("floating_point")
        .and_then(common::array)
        .is_some_and(|modes| {
            modes
                .iter()
                .any(|mode| common::scalar(mode) == Some("ieee_denormals"))
        })
    {
        required.insert("float32.denorm_preserve".to_owned());
    }
    if value("node")
        .and_then(common::object_map)
        .and_then(|node| node.get("outputs").copied())
        .and_then(common::array)
        .is_some_and(|outputs| {
            outputs.iter().any(|output| {
                common::object_map(output)
                    .and_then(|members| members.get("kind").copied())
                    .and_then(common::scalar)
                    == Some("mesh")
            })
        })
    {
        required.insert("work_graphs.mesh".to_owned());
    }
    if value("fragment")
        .and_then(common::object_map)
        .and_then(|fragment| fragment.get("reads_attachments").copied())
        .and_then(common::scalar)
        == Some("true")
    {
        required.insert("framebuffer_fetch".to_owned());
    }
    if value("stage_output")
        .and_then(common::array)
        .is_some_and(|fields| {
            fields.iter().any(|field| {
                common::object_map(field)
                    .and_then(|members| members.get("semantic").copied())
                    .and_then(common::scalar)
                    == Some("shading_rate")
            })
        })
    {
        required.insert("shading_rate".to_owned());
    }
    required
}

pub(super) struct RecordedSpecializationConstant {
    pub(super) ty: String,
    pub(super) default: String,
}

pub(super) fn normalize_text(source: &str) -> Option<String> {
    common::normalize_text(source)
}

pub(super) fn stage_field_types(entry: &TextGpuEntry, name: &str) -> Option<Vec<(Type, u64)>> {
    let value = field_value(entry, name)?;
    common::array(value)?
        .iter()
        .map(|field| {
            let members = common::object_map(field)?;
            let ty = common::scalar(members.get("type")?)?;
            let ty = tir_format::parse_tvisa_type(ty).ok()?;
            let offset = common::number(members.get("offset")?)?;
            Some((ty, offset))
        })
        .collect()
}

pub(super) fn specialization_constants(
    source: &str,
    entry: &TextGpuEntry,
) -> Option<HashMap<u64, RecordedSpecializationConstant>> {
    let value = field_value(entry, "specialization_constants")?;
    let mut constants = HashMap::new();
    for value in common::array(value)? {
        let members = common::object_map(value)?;
        let id = members.get("id").and_then(|value| common::number(value))?;
        let ty = members
            .get("type")
            .and_then(|value| common::scalar(value))?;
        let ty = tir_format::parse_tvisa_type(ty).ok()?.to_string();
        let default = members.get("default")?;
        let raw = source.get(default.span.start..default.span.end)?.trim();
        let default = common::normalize_text(raw)?;
        constants.insert(id, RecordedSpecializationConstant { ty, default });
    }
    Some(constants)
}

fn field_value<'a>(entry: &'a TextGpuEntry, name: &str) -> Option<&'a Value> {
    entry
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}
