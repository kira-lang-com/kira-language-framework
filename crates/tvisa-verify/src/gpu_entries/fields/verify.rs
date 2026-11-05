use std::collections::{HashMap, HashSet};

use tir_diag::Diagnostic;
use tir_format::parse_tvisa_type;

use super::common::{
    array, diagnostic, number, object_map, scalar, valid_semantic, verify_hex, verify_number,
    verify_object,
};
use super::parser::{Field, Value};
use super::special::{fragment, node, ray_tracing, specialization_constants};

const FIELDS: &[&str] = &[
    "threads",
    "root",
    "immediate",
    "stage_input",
    "stage_output",
    "payload",
    "mesh",
    "threadgroup_memory",
    "capabilities",
    "subgroup_size",
    "floating_point",
    "specialization_constants",
    "node",
    "ray_tracing",
    "fragment",
];
const CAPABILITIES: &[&str] = &[
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

pub(super) fn fields(fields: &[Field], stage: &str, base: usize) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let expected: &[&str] = if stage == "visible" {
        &["typeid"]
    } else {
        FIELDS
    };
    let mut values = HashMap::new();
    for field in fields {
        if !expected.contains(&field.name.as_str()) {
            diagnostics.push(diagnostic(
                base,
                field.span,
                format!("unknown field '{}' in GPU {stage} record", field.name),
            ));
        } else if values.insert(field.name.as_str(), &field.value).is_some() {
            diagnostics.push(diagnostic(
                base,
                field.span,
                format!("GPU entry field '{}' appears more than once", field.name),
            ));
        }
    }
    for name in expected {
        if !values.contains_key(name) {
            diagnostics.push(diagnostic(
                base,
                fields
                    .first()
                    .map_or(tir_diag::Span::new(0, 0), |field| field.span),
                format!("GPU {stage} record is missing field '{name}'"),
            ));
        }
    }
    if stage == "visible" {
        if let Some(value) = values.get("typeid") {
            verify_hex(value, "visible typeid", base, &mut diagnostics);
        }
        return diagnostics;
    }

    if let Some(value) = values.get("threads") {
        threads(value, stage, base, &mut diagnostics);
    }
    if let Some(value) = values.get("root") {
        optional_object(
            value,
            "root",
            &["size", "align_log2", "typeid", "space"],
            base,
            &mut diagnostics,
        );
        if let Some(root) = object_map(value) {
            if root.get("size").and_then(|size| number(size)) == Some(0) {
                diagnostics.push(diagnostic(
                    base,
                    value.span,
                    "a present root must have nonzero size; use none when absent",
                ));
            }
            if root
                .get("align_log2")
                .and_then(|align| number(align))
                .is_some_and(|align| align > 63)
            {
                diagnostics.push(diagnostic(
                    base,
                    root["align_log2"].span,
                    "root alignment exponent must be at most 63",
                ));
            }
        }
    }
    if let Some(value) = values.get("immediate") {
        optional_object(
            value,
            "immediate",
            &["size", "typeid"],
            base,
            &mut diagnostics,
        );
        if object_map(value)
            .and_then(|object| object.get("size").copied())
            .and_then(number)
            .is_some_and(|size| size > 256)
        {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "immediate data exceeds 256 bytes",
            ));
        }
        if object_map(value)
            .and_then(|object| object.get("size").copied())
            .and_then(number)
            == Some(0)
        {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "a present immediate record must have nonzero size; use none when absent",
            ));
        }
    }
    for name in ["stage_input", "stage_output"] {
        if let Some(value) = values.get(name) {
            stage_fields(value, name, stage, base, &mut diagnostics);
        }
    }
    if let Some(value) = values.get("payload") {
        optional_object(
            value,
            "payload",
            &["object", "ray", "hit_attribute", "callable"],
            base,
            &mut diagnostics,
        );
        if let Some(payload) = object_map(value) {
            let has_size = ["object", "ray", "hit_attribute", "callable"]
                .iter()
                .any(|name| {
                    payload
                        .get(name)
                        .and_then(|size| number(size))
                        .is_some_and(|n| n > 0)
                });
            if !has_size {
                diagnostics.push(diagnostic(
                    base,
                    value.span,
                    "an all-zero payload must use none",
                ));
            }
        }
    }
    if let Some(value) = values.get("mesh") {
        mesh(value, stage, base, &mut diagnostics);
    }
    if let Some(value) = values.get("threadgroup_memory") {
        verify_number(value, "threadgroup_memory", base, &mut diagnostics);
    }
    if let Some(value) = values.get("capabilities") {
        capabilities(value, base, &mut diagnostics);
    }
    if let Some(value) = values.get("subgroup_size") {
        subgroup_size(value, base, &mut diagnostics);
    }
    if let Some(value) = values.get("floating_point") {
        floating_point(value, base, &mut diagnostics);
    }
    if let Some(value) = values.get("specialization_constants") {
        specialization_constants(value, base, &mut diagnostics);
    }
    if let Some(value) = values.get("node") {
        node(value, stage, base, &mut diagnostics);
    }
    if let Some(value) = values.get("ray_tracing") {
        ray_tracing(value, stage, base, &mut diagnostics);
    }
    if let Some(value) = values.get("fragment") {
        fragment(value, stage, base, &mut diagnostics);
    }
    diagnostics
}

fn threads(value: &Value, stage: &str, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "threads must be a three-element array",
        ));
        return;
    };
    if values.len() != 3 {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "threads must contain exactly three counts",
        ));
        return;
    }
    let counts: Vec<_> = values.iter().filter_map(number).collect();
    if counts.len() != 3 {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "thread counts must be nonnegative integers",
        ));
        return;
    }
    let threaded = matches!(stage, "compute" | "object" | "mesh" | "tile");
    if !threaded && counts != [0, 0, 0] {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{stage} entries do not have a threadgroup size"),
        ));
    } else if threaded && counts == [0, 0, 0] && stage != "compute" {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{stage} entries require fixed thread counts"),
        ));
    } else if threaded && counts.iter().any(|count| *count == 0) && counts != [0, 0, 0] {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "fixed thread counts must all be positive",
        ));
    }
    if stage == "tile" && counts != [0, 0, 0] && counts[2] != 1 {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "tile entries require a z thread count of 1",
        ));
    }
}

fn optional_object(
    value: &Value,
    name: &str,
    members: &[&str],
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if scalar(value) == Some("none") {
        return;
    }
    verify_object(value, name, members, base, diagnostics);
    let Some(object) = object_map(value) else {
        return;
    };
    for member in members {
        let Some(value) = object.get(member) else {
            continue;
        };
        match *member {
            "typeid" => verify_hex(value, &format!("{name}.{member}"), base, diagnostics),
            "size" | "align_log2" | "object" | "ray" | "hit_attribute" | "callable" => {
                verify_number(value, &format!("{name}.{member}"), base, diagnostics);
            }
            "space" if !matches!(scalar(value), Some("device" | "constant")) => {
                diagnostics.push(diagnostic(
                    base,
                    value.span,
                    "root space must be device or constant",
                ));
            }
            _ => {}
        }
    }
}

fn stage_fields(
    value: &Value,
    name: &str,
    stage: &str,
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{name} must be an array"),
        ));
        return;
    };
    if name == "stage_input" && stage != "fragment" && !values.is_empty() {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{stage} entries cannot declare stage inputs"),
        ));
    }
    if name == "stage_output"
        && !matches!(stage, "vertex" | "mesh" | "fragment")
        && !values.is_empty()
    {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("{stage} entries cannot declare stage outputs"),
        ));
    }
    if stage == "vertex"
        && name == "stage_output"
        && !values.iter().any(|field| {
            object_map(field)
                .and_then(|object| object.get("semantic").copied())
                .and_then(scalar)
                .is_some_and(|semantic| {
                    semantic.split_whitespace().collect::<String>() == "position"
                })
        })
    {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "vertex entries require a position stage output",
        ));
    }
    let mut semantics = HashSet::new();
    for field in values {
        let members = ["semantic", "location", "type", "interpolation", "offset"];
        verify_object(field, name, &members, base, diagnostics);
        let Some(object) = object_map(field) else {
            continue;
        };
        let semantic_name = object
            .get("semantic")
            .and_then(|semantic| scalar(semantic))
            .map(|semantic| semantic.split_whitespace().collect::<String>());
        let location_number = object.get("location").and_then(|location| number(location));
        if let (Some(semantic), Some(location)) = (semantic_name.as_deref(), location_number)
            && !semantics.insert((semantic.to_owned(), location))
        {
            diagnostics.push(diagnostic(
                base,
                field.span,
                format!("duplicate {name} semantic '{semantic}' at location {location}"),
            ));
        }
        if let Some(semantic) = object.get("semantic") {
            let text = scalar(semantic)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<String>();
            if !valid_semantic(&text) {
                diagnostics.push(diagnostic(
                    base,
                    semantic.span,
                    format!("unknown stage semantic '{text}'"),
                ));
            }
            if text == "shading_rate" && name != "stage_output" {
                diagnostics.push(diagnostic(
                    base,
                    semantic.span,
                    "shading_rate is an output semantic",
                ));
            }
        }
        if let Some(location) = object.get("location") {
            verify_number(location, "stage location", base, diagnostics);
        }
        if let Some(ty) = object.get("type") {
            if !scalar(ty).is_some_and(|text| parse_tvisa_type(text).is_ok()) {
                diagnostics.push(diagnostic(
                    base,
                    ty.span,
                    "stage field type must use valid TVISA type syntax",
                ));
            }
        }
        if let Some(interpolation) = object.get("interpolation") {
            interpolations(interpolation, base, diagnostics);
        }
        if let Some(offset) = object.get("offset") {
            verify_number(offset, "stage field byte offset", base, diagnostics);
        }
    }
}

fn interpolations(value: &Value, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "interpolation must be an array",
        ));
        return;
    };
    let mut seen = HashSet::new();
    for item in values {
        let Some(name) = scalar(item) else {
            diagnostics.push(diagnostic(
                base,
                item.span,
                "interpolation values must be names",
            ));
            continue;
        };
        if !matches!(
            name,
            "flat"
                | "perspective"
                | "noperspective"
                | "centroid"
                | "sample"
                | "invariant"
                | "per_primitive"
        ) {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("unknown interpolation '{name}'"),
            ));
        }
        if !seen.insert(name) {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("duplicate interpolation '{name}'"),
            ));
        }
    }
}

fn mesh(value: &Value, stage: &str, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    if scalar(value) == Some("none") {
        if stage == "mesh" {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "mesh entries require mesh limits",
            ));
        }
        return;
    }
    if stage != "mesh" {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("mesh limits are not valid for {stage} entries"),
        ));
    }
    verify_object(
        value,
        "mesh",
        &["max_vertices", "max_primitives", "topology"],
        base,
        diagnostics,
    );
    let Some(object) = object_map(value) else {
        return;
    };
    for (name, limit) in [("max_vertices", 256), ("max_primitives", 512)] {
        if let Some(member) = object.get(name)
            && let Some(count) = number(member)
            && (count == 0 || count > limit)
        {
            diagnostics.push(diagnostic(
                base,
                member.span,
                format!("{name} must be between 1 and {limit}"),
            ));
        }
    }
    if let Some(topology) = object.get("topology")
        && !matches!(scalar(topology), Some("points" | "lines" | "triangles"))
    {
        diagnostics.push(diagnostic(
            base,
            topology.span,
            "mesh topology must be points, lines, or triangles",
        ));
    }
}

fn capabilities(value: &Value, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "capabilities must be an array",
        ));
        return;
    };
    let mut seen = HashSet::new();
    for item in values {
        let Some(name) = scalar(item) else {
            diagnostics.push(diagnostic(
                base,
                item.span,
                "capability names must be scalars",
            ));
            continue;
        };
        if !CAPABILITIES.contains(&name) {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("unknown optional GPU capability '{name}'"),
            ));
        }
        if !seen.insert(name) {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("duplicate GPU capability '{name}'"),
            ));
        }
    }
}

fn subgroup_size(value: &Value, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    if scalar(value) == Some("any") {
        return;
    }
    verify_number(value, "subgroup_size", base, diagnostics);
    if number(value).is_some_and(|size| !(4..=128).contains(&size) || !size.is_power_of_two()) {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "subgroup_size must be any or a power of two from 4 through 128",
        ));
    }
}

fn floating_point(value: &Value, base: usize, diagnostics: &mut Vec<Diagnostic>) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "floating_point must be an array",
        ));
        return;
    };
    let mut seen = HashSet::new();
    for item in values {
        let Some(name) = scalar(item) else {
            diagnostics.push(diagnostic(
                base,
                item.span,
                "floating-point modes must be names",
            ));
            continue;
        };
        if !matches!(name, "fast" | "ieee_denormals") {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("unknown floating-point mode '{name}'"),
            ));
        }
        if !seen.insert(name) {
            diagnostics.push(diagnostic(
                base,
                item.span,
                format!("duplicate floating-point mode '{name}'"),
            ));
        }
    }
}
