use std::collections::HashSet;

use tir_format::parse_tvisa_type;

use super::common::{
    array, diagnostic, number, object_map, scalar, string_value, verify_number, verify_object,
};
use super::parser::Value;

pub(super) fn specialization_constants(
    value: &Value,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    let Some(values) = array(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "specialization_constants must be an array",
        ));
        return;
    };
    let mut ids = HashSet::new();
    for item in values {
        verify_object(
            item,
            "specialization constant",
            &["id", "type", "default"],
            base,
            diagnostics,
        );
        let Some(object) = object_map(item) else {
            continue;
        };
        if let Some(id) = object.get("id") {
            if let Some(id) = number(id) {
                if id > u64::from(u32::MAX) {
                    diagnostics.push(diagnostic(
                        base,
                        object["id"].span,
                        "specialization constant id exceeds 32 bits",
                    ));
                }
                if !ids.insert(id) {
                    diagnostics.push(diagnostic(
                        base,
                        item.span,
                        format!("duplicate specialization constant id {id}"),
                    ));
                }
            } else {
                verify_number(id, "specialization constant id", base, diagnostics);
            }
        }
        if object.get("type").is_some_and(|ty| {
            scalar(ty)
                .and_then(|text| parse_tvisa_type(text).ok())
                .is_none()
        }) {
            let ty = object["type"];
            diagnostics.push(diagnostic(
                base,
                ty.span,
                "specialization constant requires a valid TVISA type",
            ));
        }
        if object
            .get("default")
            .is_some_and(|default| scalar(default).is_none() && array(default).is_none())
        {
            let default = object["default"];
            diagnostics.push(diagnostic(
                base,
                default.span,
                "specialization constant default must be a scalar or array literal",
            ));
        }
    }
}

pub(super) fn node(
    value: &Value,
    stage: &str,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    if scalar(value) == Some("none") {
        if stage == "node" {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "node entries require node metadata",
            ));
        }
        return;
    }
    if stage != "node" {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("node metadata is not valid for {stage} entries"),
        ));
    }
    verify_object(
        value,
        "node",
        &[
            "launch",
            "max_dispatch_grid",
            "input_record_size",
            "outputs",
        ],
        base,
        diagnostics,
    );
    let Some(object) = object_map(value) else {
        return;
    };
    if let Some(launch) = object.get("launch")
        && !matches!(scalar(launch), Some("broadcast" | "coalescing" | "thread"))
    {
        diagnostics.push(diagnostic(
            base,
            launch.span,
            "node launch must be broadcast, coalescing, or thread",
        ));
    }
    if let Some(grid) = object.get("max_dispatch_grid") {
        if let Some(dimensions) = array(grid) {
            if dimensions.len() != 3 {
                diagnostics.push(diagnostic(
                    base,
                    grid.span,
                    "max_dispatch_grid must have three dimensions",
                ));
            }
            for dimension in dimensions {
                verify_number(dimension, "node dispatch-grid dimension", base, diagnostics);
            }
        } else {
            diagnostics.push(diagnostic(
                base,
                grid.span,
                "max_dispatch_grid must be an array",
            ));
        }
    }
    if let Some(size) = object.get("input_record_size") {
        verify_number(size, "node input record size", base, diagnostics);
    }
    if let Some(outputs) = object.get("outputs") {
        let Some(outputs) = array(outputs) else {
            diagnostics.push(diagnostic(
                base,
                outputs.span,
                "node outputs must be an array",
            ));
            return;
        };
        let mut names = HashSet::new();
        for output in outputs {
            verify_object(
                output,
                "node output",
                &["name", "record_size", "max_records"],
                base,
                diagnostics,
            );
            let Some(members) = object_map(output) else {
                continue;
            };
            if let Some(name) = members.get("name") {
                let text = match string_value(name) {
                    Ok(Some(text)) => text,
                    Ok(None) => {
                        diagnostics.push(diagnostic(
                            base,
                            name.span,
                            "node output name must be a string",
                        ));
                        continue;
                    }
                    Err(error) => {
                        diagnostics.push(diagnostic(
                            base,
                            name.span,
                            format!("node output name has an invalid string escape: {error}"),
                        ));
                        continue;
                    }
                };
                if !names.insert(text.clone()) {
                    diagnostics.push(diagnostic(
                        base,
                        output.span,
                        format!("duplicate node output '{text}'"),
                    ));
                }
            }
            for field in ["record_size", "max_records"] {
                if let Some(size) = members.get(field) {
                    verify_number(size, &format!("node output {field}"), base, diagnostics);
                }
            }
        }
    }
}

pub(super) fn ray_tracing(
    value: &Value,
    stage: &str,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    let ray_stage = matches!(
        stage,
        "raygen" | "intersection" | "anyhit" | "closesthit" | "miss" | "callable"
    );
    if scalar(value) == Some("none") {
        if ray_stage {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "ray-tracing entries require recursion metadata",
            ));
        }
        return;
    }
    if !ray_stage {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("ray-tracing metadata is not valid for {stage} entries"),
        ));
    }
    verify_object(
        value,
        "ray_tracing",
        &["max_recursion_depth"],
        base,
        diagnostics,
    );
    if let Some(depth) =
        object_map(value).and_then(|object| object.get("max_recursion_depth").copied())
    {
        verify_number(depth, "maximum ray recursion depth", base, diagnostics);
        if number(depth).is_some_and(|depth| depth > 31) {
            diagnostics.push(diagnostic(
                base,
                depth.span,
                "maximum ray recursion depth exceeds 31",
            ));
        }
    }
}

pub(super) fn fragment(
    value: &Value,
    stage: &str,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    if scalar(value) == Some("none") {
        if stage == "fragment" {
            diagnostics.push(diagnostic(
                base,
                value.span,
                "fragment entries require fragment metadata",
            ));
        }
        return;
    }
    if stage != "fragment" {
        diagnostics.push(diagnostic(
            base,
            value.span,
            format!("fragment metadata is not valid for {stage} entries"),
        ));
    }
    verify_object(
        value,
        "fragment",
        &[
            "early_fragment_tests",
            "writes_depth",
            "discards_or_demotes",
            "reads_attachments",
        ],
        base,
        diagnostics,
    );
    let Some(object) = object_map(value) else {
        return;
    };
    for field in [
        "early_fragment_tests",
        "discards_or_demotes",
        "reads_attachments",
    ] {
        if let Some(value) = object.get(field)
            && !matches!(scalar(value), Some("true" | "false"))
        {
            diagnostics.push(diagnostic(
                base,
                value.span,
                format!("fragment.{field} must be boolean"),
            ));
        }
    }
    if let Some(value) = object.get("writes_depth")
        && !matches!(scalar(value), Some("none" | "any" | "greater" | "less"))
    {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "fragment.writes_depth must be none, any, greater, or less",
        ));
    }
}
