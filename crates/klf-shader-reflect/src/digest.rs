//! The `.resources` digest (section 21.3), byte for byte the oracle's.

use crate::model::{Reflection, ResourceRecord};
use klf_shader::{Stage, Target};
use std::fmt::Write;

fn binding(resource: &ResourceRecord, target: Target) -> u32 {
    resource
        .binding(target)
        .map_or(0, |binding| binding.binding)
}

fn scalar_kind(type_name: &str) -> char {
    match type_name {
        "Int" | "Int2" | "Int3" | "Int4" | "Bool" => 'i',
        "UInt" | "UInt2" | "UInt3" | "UInt4" => 'u',
        _ => 'f',
    }
}

fn natural_size(type_name: &str) -> u32 {
    match type_name {
        "Float" | "Int" | "UInt" | "Bool" => 4,
        "Float2" | "Int2" | "UInt2" => 8,
        "Float3" | "Int3" | "UInt3" => 12,
        "Float4" | "Int4" | "UInt4" => 16,
        "Float2x2" => 32,
        "Float3x3" => 48,
        "Float4x4" => 64,
        _ => 16,
    }
}

/// The digest of every resource of `reflection`.
#[must_use]
pub fn resource_digest(reflection: &Reflection) -> String {
    let mut out = String::new();
    for resource in &reflection.resources {
        let at = binding(resource, Target::Wgsl);
        let group = resource
            .binding(Target::Wgsl)
            .map_or(0, |binding| binding.group);
        let mask: u32 = resource
            .visibility
            .iter()
            .map(|stage| match stage {
                Stage::Vertex => 1,
                Stage::Fragment => 2,
                Stage::Compute => 4,
            })
            .fold(0, |mask, bit| mask | bit);
        let name = &resource.name;
        match resource.kind.as_str() {
            "uniform" => {
                let Some(declared) = reflection
                    .types
                    .iter()
                    .find(|declared| declared.name == resource.type_name)
                else {
                    continue;
                };
                let Some(layout) = &declared.layout else {
                    continue;
                };
                let _ = write!(
                    out,
                    "u|{name}:{at}:{}:{mask}:{}",
                    layout.size,
                    layout.fields.len()
                );
                let mut kinds = String::new();
                for (index, field) in layout.fields.iter().enumerate() {
                    out.push(if index == 0 { ':' } else { ',' });
                    let member = declared
                        .fields
                        .iter()
                        .find(|member| member.name == field.name);
                    let natural =
                        member.map_or(field.size, |member| natural_size(&member.type_name));
                    kinds.push(member.map_or('f', |member| scalar_kind(&member.type_name)));
                    let _ = write!(out, "{}@{}#{natural}", field.name, field.offset);
                }
                let _ = write!(out, ":{kinds}:{group}");
            }
            "storage" => {
                let readonly = u32::from(resource.access.as_deref() != Some("read_write"));
                let _ = write!(
                    out,
                    "s|{name}:{at}:{mask}:{}:{readonly}:{group}",
                    binding(resource, Target::Glsl)
                );
            }
            "texture" if matches!(resource.access.as_deref(), Some("write" | "read_write")) => {
                let writeonly = u32::from(resource.access.as_deref() == Some("write"));
                let _ = write!(
                    out,
                    "i|{name}:{at}:{mask}:{}:{writeonly}:{group}",
                    binding(resource, Target::Glsl)
                );
            }
            "texture" => {
                let sampler = resource
                    .paired_sampler
                    .as_ref()
                    .and_then(|paired| {
                        reflection
                            .resources
                            .iter()
                            .find(|other| &other.name == paired)
                    })
                    .map_or(255, |other| binding(other, Target::Wgsl));
                let glsl_name = resource
                    .binding(Target::Glsl)
                    .and_then(|binding| binding.glsl_name.clone())
                    .unwrap_or_else(|| name.clone());
                let _ = write!(out, "t|{name}:{at}:{mask}:{sampler}:{glsl_name}:{group}");
            }
            _ => {
                let _ = write!(out, "m|{name}:{at}:{mask}:{group}");
            }
        }
        out.push(';');
    }
    out
}
