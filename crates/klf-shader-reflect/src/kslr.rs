//! `KSLR1`, the oracle's reflection text (section 21.2), with KLF's
//! appended `source`, `spec` and `rename` records.

use crate::model::{
    BindRecord, FieldRecord, Layout, LayoutField, OptionRecord, Reflection, Rename, ResourceRecord,
    StageRecord, TypeRecord,
};
use klf_shader::{Stage, Target};
use std::fmt::Write;

pub const MAGIC: &str = "KSLR1";

fn optional(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn visibility(stages: &[Stage]) -> String {
    if stages.is_empty() {
        return "-".to_owned();
    }
    stages
        .iter()
        .map(|stage| stage.name())
        .collect::<Vec<_>>()
        .join(",")
}

fn interface_line(out: &mut String, keyword: &str, stage: Stage, field: &FieldRecord) {
    let _ = writeln!(
        out,
        "{keyword} {} {} {} {} {} {}",
        stage.name(),
        field
            .location
            .map_or_else(|| "-".to_owned(), |at| at.to_string()),
        field.name,
        field.type_name,
        optional(field.builtin.as_deref()),
        optional(field.interpolation.as_deref()),
    );
}

/// Renders `reflection` as a `KSLR1` document.
#[must_use]
pub fn encode(reflection: &Reflection) -> String {
    let mut out = format!("{MAGIC}\n");
    let _ = writeln!(
        out,
        "shader {} {} {}",
        reflection.shader_name,
        reflection.shader_kind,
        reflection.target.map_or("-", Target::name)
    );
    if !reflection.source.is_empty() {
        let _ = writeln!(out, "source {}", reflection.source);
    }
    for option in &reflection.options {
        let _ = writeln!(
            out,
            "option {} {} {}",
            option.name, option.type_name, option.value
        );
        let _ = writeln!(
            out,
            "spec {} {} {}",
            option.name,
            option
                .id
                .map_or_else(|| "-".to_owned(), |id| id.to_string()),
            if option.keep { "keep" } else { "bake" }
        );
    }
    for stage in &reflection.stages {
        let _ = writeln!(
            out,
            "stage {} entry {} input {} output {}",
            stage.stage.name(),
            stage.entry_name,
            optional(stage.input_type.as_deref()),
            optional(stage.output_type.as_deref())
        );
        if let Some([x, y, z]) = stage.threads {
            let _ = writeln!(out, "threads {x} {y} {z}");
        }
        for field in &stage.inputs {
            interface_line(&mut out, "in", stage.stage, field);
        }
        for field in &stage.outputs {
            interface_line(&mut out, "out", stage.stage, field);
        }
    }
    for resource in &reflection.resources {
        let _ = writeln!(
            out,
            "resource {} {} {} {} {} {} {} {}",
            resource.group_name,
            resource.group_class,
            resource.group_index,
            resource.name,
            resource.kind,
            resource.type_name,
            optional(resource.access.as_deref()),
            visibility(&resource.visibility),
        );
        for binding in &resource.bindings {
            let _ = writeln!(
                out,
                "bind {} {} {} {} {}",
                resource.name,
                binding.target.name(),
                binding.group,
                binding.binding,
                optional(binding.glsl_name.as_deref())
            );
        }
        if let Some(sampler) = &resource.paired_sampler {
            let _ = writeln!(out, "pair {} {sampler}", resource.name);
        }
        for (target, binding) in &resource.lengths {
            let _ = writeln!(out, "count {} {} {binding}", resource.name, target.name());
        }
    }
    for declared in &reflection.types {
        for field in &declared.fields {
            let _ = writeln!(
                out,
                "member {} {} {} {} {}",
                declared.name,
                field.name,
                field.type_name,
                optional(field.builtin.as_deref()),
                optional(field.interpolation.as_deref()),
            );
        }
        if let Some(layout) = &declared.layout {
            let _ = writeln!(
                out,
                "layout {} {} {}",
                declared.name, layout.alignment, layout.size
            );
            for field in &layout.fields {
                let _ = writeln!(
                    out,
                    "field {} {} {} {} {} {}",
                    declared.name,
                    field.name,
                    field.offset,
                    field.alignment,
                    field.size,
                    field.stride
                );
            }
        }
    }
    for rename in &reflection.renames {
        let _ = writeln!(out, "rename {} {} {}", rename.kind, rename.from, rename.to);
    }
    out
}

fn some(value: &str) -> Option<String> {
    (value != "-").then(|| value.to_owned())
}

fn number(value: &str, line: usize) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("line {line}: `{value}` is not a number"))
}

fn stage(value: &str, line: usize) -> Result<Stage, String> {
    Stage::from_name(value).ok_or_else(|| format!("line {line}: `{value}` is not a stage"))
}

fn target(value: &str, line: usize) -> Result<Target, String> {
    Target::from_name(value).ok_or_else(|| format!("line {line}: `{value}` is not a target"))
}

fn field_record(fields: &[&str], line: usize) -> Result<FieldRecord, String> {
    Ok(FieldRecord {
        name: fields[2].to_owned(),
        type_name: fields[3].to_owned(),
        builtin: some(fields[4]),
        interpolation: some(fields[5]),
        location: some(fields[1])
            .map(|value| number(&value, line))
            .transpose()?,
    })
}

/// Decodes a `KSLR1` document. Unknown records and wrong field counts are
/// errors, never skipped.
///
/// # Errors
/// Returns `line N: why`.
pub fn decode(text: &str) -> Result<Reflection, String> {
    let mut lines = text.lines().enumerate();
    match lines.next() {
        Some((_, MAGIC)) => {}
        other => {
            return Err(format!(
                "expected `{MAGIC}`, found {:?}",
                other.map(|(_, line)| line)
            ));
        }
    }
    let mut reflection = Reflection::default();
    for (index, line) in lines {
        let at = index + 1;
        let fields: Vec<&str> = line.split(' ').collect();
        let wanted = match fields[0] {
            "shader" => 4,
            "source" => 2,
            "option" | "spec" | "pair" | "count" | "rename" | "layout" => 4,
            "stage" => 8,
            "threads" => 4,
            "in" | "out" => 7,
            "resource" => 9,
            "bind" => 6,
            "member" => 6,
            "field" => 7,
            keyword => {
                return Err(format!(
                    "line {at}: `{keyword}` is not a record this format defines"
                ));
            }
        };
        if fields.len() != wanted {
            return Err(format!(
                "line {at}: `{}` takes {wanted} fields, found {}",
                fields[0],
                fields.len()
            ));
        }
        record(&mut reflection, &fields, at)?;
    }
    if reflection.shader_name.is_empty() {
        return Err("no `shader` record".to_owned());
    }
    Ok(reflection)
}

fn record(reflection: &mut Reflection, fields: &[&str], at: usize) -> Result<(), String> {
    let unanchored =
        |name: &str| format!("line {at}: `{name}` was not declared before it was referred to");
    match fields[0] {
        "shader" => {
            fields[1].clone_into(&mut reflection.shader_name);
            fields[2].clone_into(&mut reflection.shader_kind);
            reflection.target = some(fields[3])
                .map(|value| target(&value, at))
                .transpose()?;
        }
        "source" => fields[1].clone_into(&mut reflection.source),
        "option" => reflection.options.push(OptionRecord {
            name: fields[1].to_owned(),
            type_name: fields[2].to_owned(),
            value: fields[3].to_owned(),
            id: None,
            keep: false,
        }),
        "spec" => {
            let option = reflection
                .options
                .iter_mut()
                .find(|option| option.name == fields[1])
                .ok_or_else(|| unanchored(fields[1]))?;
            option.id = some(fields[2])
                .map(|id| id.parse().map_err(|_| format!("line {at}: bad id")))
                .transpose()?;
            option.keep = fields[3] == "keep";
        }
        "stage" => reflection.stages.push(StageRecord {
            stage: stage(fields[1], at)?,
            entry_name: fields[3].to_owned(),
            input_type: some(fields[5]),
            output_type: some(fields[7]),
            threads: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }),
        "threads" => {
            let last = reflection
                .stages
                .last_mut()
                .ok_or_else(|| unanchored("stage"))?;
            last.threads = Some([
                number(fields[1], at)?,
                number(fields[2], at)?,
                number(fields[3], at)?,
            ]);
        }
        "in" | "out" => {
            let field = field_record(fields, at)?;
            let last = reflection
                .stages
                .last_mut()
                .ok_or_else(|| unanchored("stage"))?;
            if fields[0] == "in" {
                last.inputs.push(field);
            } else {
                last.outputs.push(field);
            }
        }
        "resource" => reflection.resources.push(ResourceRecord {
            group_name: fields[1].to_owned(),
            group_class: fields[2].to_owned(),
            group_index: number(fields[3], at)?,
            name: fields[4].to_owned(),
            kind: fields[5].to_owned(),
            type_name: fields[6].to_owned(),
            access: some(fields[7]),
            visibility: match fields[8] {
                "-" => Vec::new(),
                list => list
                    .split(',')
                    .map(|word| stage(word, at))
                    .collect::<Result<_, _>>()?,
            },
            bindings: Vec::new(),
            paired_sampler: None,
            lengths: Vec::new(),
        }),
        "bind" | "pair" | "count" => {
            let resource = reflection
                .resources
                .iter_mut()
                .find(|resource| resource.name == fields[1])
                .ok_or_else(|| unanchored(fields[1]))?;
            match fields[0] {
                "bind" => resource.bindings.push(BindRecord {
                    target: target(fields[2], at)?,
                    group: number(fields[3], at)?,
                    binding: number(fields[4], at)?,
                    glsl_name: some(fields[5]),
                }),
                "pair" => resource.paired_sampler = Some(fields[2].to_owned()),
                _ => resource
                    .lengths
                    .push((target(fields[2], at)?, number(fields[3], at)?)),
            }
        }
        "member" => {
            let field = FieldRecord {
                name: fields[2].to_owned(),
                type_name: fields[3].to_owned(),
                builtin: some(fields[4]),
                interpolation: some(fields[5]),
                location: None,
            };
            declared(reflection, fields[1]).fields.push(field);
        }
        "layout" => {
            declared(reflection, fields[1]).layout = Some(Layout {
                alignment: number(fields[2], at)?,
                size: number(fields[3], at)?,
                fields: Vec::new(),
            });
        }
        "field" => {
            let field = LayoutField {
                name: fields[2].to_owned(),
                offset: number(fields[3], at)?,
                alignment: number(fields[4], at)?,
                size: number(fields[5], at)?,
                stride: number(fields[6], at)?,
            };
            declared(reflection, fields[1])
                .layout
                .as_mut()
                .ok_or_else(|| unanchored("layout"))?
                .fields
                .push(field);
        }
        _ => reflection.renames.push(Rename {
            kind: fields[1].to_owned(),
            from: fields[2].to_owned(),
            to: fields[3].to_owned(),
        }),
    }
    Ok(())
}

fn declared<'a>(reflection: &'a mut Reflection, name: &str) -> &'a mut TypeRecord {
    if let Some(index) = reflection
        .types
        .iter()
        .position(|declared| declared.name == name)
    {
        return &mut reflection.types[index];
    }
    reflection.types.push(TypeRecord {
        name: name.to_owned(),
        fields: Vec::new(),
        layout: None,
    });
    let last = reflection.types.len() - 1;
    &mut reflection.types[last]
}
