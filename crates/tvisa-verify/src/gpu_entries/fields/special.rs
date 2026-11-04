use std::collections::HashSet;

use tir_format::parse_tvisa_type;
use tir_types::{FloatType, Type};

use super::Value;
use super::common::{
    array, diagnostic, number, object_map, scalar, string_value, verify_number, verify_object,
};

pub(super) fn specialization_constants(
    value: &Value,
    has_float64: bool,
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
        let specialization_type = object
            .get("type")
            .and_then(|ty| scalar(ty).and_then(|text| parse_tvisa_type(text).ok()));
        if let Some(ty) = specialization_type {
            if !valid_specialization_type(&ty, has_float64) {
                let type_value = object["type"];
                diagnostics.push(diagnostic(
                    base,
                    type_value.span,
                    "specialization constant has an unsupported type or lacks the float64 capability",
                ));
            } else if let Some(default) = object.get("default") {
                verify_specialization_default(default, &ty, base, diagnostics);
            }
        } else if let Some(type_value) = object.get("type") {
            diagnostics.push(diagnostic(
                base,
                type_value.span,
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

fn valid_specialization_type(ty: &Type, has_float64: bool) -> bool {
    match ty {
        Type::Integer(bits) => (1..=64).contains(bits),
        Type::Float(float) => match float {
            FloatType::F16 | FloatType::Bf16 | FloatType::F32 => true,
            FloatType::F64 => has_float64,
            _ => false,
        },
        Type::FixedVector { lanes, element } if matches!(lanes, 2..=4) => {
            valid_vector_element(element, has_float64)
        }
        _ => false,
    }
}

fn valid_vector_element(ty: &Type, has_float64: bool) -> bool {
    match ty {
        Type::Integer(bits) => (1..=64).contains(bits),
        Type::Float(float) => match float {
            FloatType::F16 | FloatType::Bf16 | FloatType::F32 => true,
            FloatType::F64 => has_float64,
            FloatType::F8E4M3
            | FloatType::F8E5M2
            | FloatType::F6E2M3
            | FloatType::F6E3M2
            | FloatType::F4E2M1
            | FloatType::E8M0 => true,
            FloatType::F128 => false,
        },
        _ => false,
    }
}

fn verify_specialization_default(
    default: &Value,
    ty: &Type,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    if let Type::FixedVector { lanes, element } = ty {
        let Some(values) = array(default) else {
            diagnostics.push(diagnostic(
                base,
                default.span,
                "fixed-vector specialization default must be a bracketed list",
            ));
            return;
        };
        if values.len() != *lanes as usize {
            diagnostics.push(diagnostic(
                base,
                default.span,
                format!("fixed-vector specialization default must have {lanes} lanes"),
            ));
            return;
        }
        for value in values {
            verify_specialization_scalar(value, element, base, diagnostics);
        }
        return;
    }

    let Some(_) = scalar(default) else {
        diagnostics.push(diagnostic(
            base,
            default.span,
            "scalar specialization default must be a scalar literal",
        ));
        return;
    };
    verify_specialization_scalar(default, ty, base, diagnostics);
}

fn verify_specialization_scalar(
    value: &Value,
    ty: &Type,
    base: usize,
    diagnostics: &mut Vec<tir_diag::Diagnostic>,
) {
    let Some(literal) = scalar(value) else {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "specialization vector lanes must be scalar literals",
        ));
        return;
    };
    let valid = match ty {
        Type::Integer(1) => matches!(literal, "true" | "false"),
        Type::Integer(bits) => tir_format::numeric_literals::integer_literal_fits(literal, *bits),
        Type::Float(float) => {
            tir_format::numeric_literals::float_literal_is_exactly_representable(literal, *float)
        }
        _ => false,
    };
    if !valid {
        diagnostics.push(diagnostic(
            base,
            value.span,
            "specialization default is out of range or not exactly representable by its type",
        ));
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
                if number(dimension) == Some(0) {
                    diagnostics.push(diagnostic(
                        base,
                        dimension.span,
                        "node dispatch-grid dimensions must be positive",
                    ));
                }
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
                &["name", "kind", "record_size", "max_records"],
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
                if text.is_empty() {
                    diagnostics.push(diagnostic(
                        base,
                        name.span,
                        "node output name must be nonempty",
                    ));
                }
                if !names.insert(text.clone()) {
                    diagnostics.push(diagnostic(
                        base,
                        output.span,
                        format!("duplicate node output '{text}'"),
                    ));
                }
            }
            if let Some(kind) = members.get("kind")
                && !matches!(scalar(kind), Some("node" | "mesh"))
            {
                diagnostics.push(diagnostic(
                    base,
                    kind.span,
                    "node output kind must be node or mesh",
                ));
            }
            for field in ["record_size", "max_records"] {
                if let Some(size) = members.get(field) {
                    verify_number(size, &format!("node output {field}"), base, diagnostics);
                    if number(size) == Some(0) {
                        diagnostics.push(diagnostic(
                            base,
                            size.span,
                            format!("node output {field} must be positive"),
                        ));
                    }
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
