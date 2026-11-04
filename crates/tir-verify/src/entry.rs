use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextItemKind, TextModule, parse_function_signature};
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, Type};

use crate::gpu_profile::policy::TypePolicies;

mod attributes;
mod node_attributes;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

#[derive(Clone, Copy)]
struct Parameter<'a> {
    kind: &'a str,
    span: Span,
}

pub(super) fn verify(
    module: &TextModule,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Entry)
    {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        let header_end = tokens
            .iter()
            .position(|token| punctuation(token, '{'))
            .unwrap_or(tokens.len());
        let header = tokens[..header_end]
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .cloned()
            .collect::<Vec<_>>();
        let Some(stage) = header.get(1).and_then(atom) else {
            continue;
        };
        let mut item_diagnostics = Vec::new();
        if !is_entry_stage(stage) {
            item_diagnostics.push(
                error(format!("unknown GPU entry kind '{stage}'"))
                    .with_primary_span(header[1].span),
            );
        }
        let parameters = entry_parameters(&header);
        let signature = parse_function_signature(source).ok().flatten();
        item_diagnostics.extend(verify_parameters(
            stage,
            &parameters,
            signature
                .as_ref()
                .map(|signature| signature.parameters.as_slice()),
            layouts,
            policies,
        ));
        if let Some(signature) = signature.as_ref() {
            item_diagnostics.extend(verify_result(
                stage,
                &signature.result,
                &header,
                layouts,
                policies,
            ));
        }
        item_diagnostics.extend(attributes::verify(
            source, stage, &header, layouts, policies,
        ));
        for diagnostic in &mut item_diagnostics {
            shift(diagnostic, item.span.start);
        }
        diagnostics.extend(item_diagnostics);
    }
    diagnostics
}

fn verify_parameters(
    stage: &str,
    parameters: &[Parameter<'_>],
    types: Option<&[tir_format::TextParameter]>,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for parameter in parameters {
        let count = counts.entry(parameter.kind).or_default();
        *count += 1;
        if *count > 1 {
            diagnostics.push(
                error(format!(
                    "GPU entry has multiple '{}' parameters",
                    parameter.kind
                ))
                .with_primary_span(parameter.span),
            );
        }
    }
    let (required, optional) = stage_parameters(stage);
    for kind in required {
        if !counts.contains_key(kind) {
            diagnostics.push(error(format!(
                "{stage} entry requires a '{kind}' parameter"
            )));
        }
    }
    let allowed = ["root", "immediate"]
        .into_iter()
        .chain(required.iter().copied())
        .chain(optional.iter().copied())
        .collect::<Vec<_>>();
    for parameter in parameters {
        if !allowed.contains(&parameter.kind) {
            diagnostics.push(
                error(format!(
                    "'{}' is not a parameter kind for {stage} entries",
                    parameter.kind
                ))
                .with_primary_span(parameter.span),
            );
        }
    }
    if let Some(types) = types {
        for (parameter, typed) in parameters.iter().zip(types) {
            diagnostics.extend(verify_parameter_type(
                parameter.kind,
                stage,
                &typed.ty,
                typed.span,
                layouts,
                policies,
            ));
        }
    }
    diagnostics
}

fn stage_parameters(stage: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match stage {
        "object" => (&["payload"], &[]),
        "mesh" => (&[], &["payload"]),
        "fragment" => (&["input"], &[]),
        "anyhit" | "closesthit" => (&["payload", "attrs"], &[]),
        "miss" => (&["payload"], &[]),
        "callable" => (&["data"], &[]),
        "node" => (&["record"], &[]),
        _ => (&[], &[]),
    }
}

fn verify_parameter_type(
    kind: &str,
    stage: &str,
    ty: &Type,
    span: Span,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match kind {
        "root" => {
            let Type::Address { pointee, space } = ty else {
                return vec![type_error("root must be a pointer to a struct", span)];
            };
            if !matches!(space, Some(AddressSpace::Constant | AddressSpace::Device)) {
                diagnostics.push(type_error(
                    "root must use the constant or device address space",
                    span,
                ));
            }
            require_struct_layout(
                pointee,
                "root pointee",
                span,
                layouts,
                policies,
                &mut diagnostics,
            );
        }
        "input" => require_struct_layout(
            ty,
            "entry parameter",
            span,
            layouts,
            policies,
            &mut diagnostics,
        ),
        "immediate" => {
            require_struct(ty, "entry parameter", span, policies, &mut diagnostics);
            if !policies.is_trivial(ty) {
                diagnostics.push(type_error("immediate data must be trivial", span));
            }
            match layouts.layout(ty) {
                Ok(layout) if layout.size > 256 => diagnostics.push(type_error(
                    "immediate data exceeds the 256-byte limit",
                    span,
                )),
                Err(layout_error) => diagnostics.push(type_error(
                    format!("immediate data has no computable layout: {layout_error}"),
                    span,
                )),
                _ => {}
            }
        }
        "payload" => {
            let expected = if matches!(stage, "object" | "mesh") {
                AddressSpace::ObjectPayload
            } else {
                AddressSpace::RayPayload
            };
            check_pointer_struct(
                ty,
                expected,
                "payload",
                span,
                layouts,
                policies,
                &mut diagnostics,
            );
        }
        "attrs" => check_pointer_struct(
            ty,
            AddressSpace::HitAttribute,
            "hit attributes",
            span,
            layouts,
            policies,
            &mut diagnostics,
        ),
        "data" => check_pointer_struct(
            ty,
            AddressSpace::RayPayload,
            "callable data",
            span,
            layouts,
            policies,
            &mut diagnostics,
        ),
        "record" => check_pointer_struct(
            ty,
            AddressSpace::NodeRecord,
            "node record",
            span,
            layouts,
            policies,
            &mut diagnostics,
        ),
        _ => {}
    }
    diagnostics
}

fn check_pointer_struct(
    ty: &Type,
    expected: AddressSpace,
    name: &str,
    span: Span,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Type::Address { pointee, space } = ty else {
        diagnostics.push(type_error(
            format!("{name} must be a pointer to a struct"),
            span,
        ));
        return;
    };
    if *space != Some(expected) {
        diagnostics.push(type_error(
            format!("{name} must use the {expected:?} address space"),
            span,
        ));
    }
    require_struct_layout(pointee, name, span, layouts, policies, diagnostics);
}

fn require_struct(
    ty: &Type,
    name: &str,
    span: Span,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !policies.is_struct(ty) {
        diagnostics.push(type_error(
            format!("{name} must be a defined struct type"),
            span,
        ));
    }
}

fn require_struct_layout(
    ty: &Type,
    name: &str,
    span: Span,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    require_struct(ty, name, span, policies, diagnostics);
    if !policies.is_struct(ty) {
        return;
    }
    if let Err(layout_error) = layouts.layout(ty) {
        diagnostics.push(type_error(
            format!("{name} has no computable layout: {layout_error}"),
            span,
        ));
    }
}

fn verify_result(
    stage: &str,
    result: &Type,
    header: &[Token],
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match stage {
        "vertex" | "fragment" => {
            require_struct_layout(
                result,
                "stage result",
                header.first().map_or(Span::new(0, 0), |token| token.span),
                layouts,
                policies,
                &mut diagnostics,
            );
            if stage == "vertex" && !policies.has_field_annotation(result, "position") {
                diagnostics.push(error("vertex result struct must have a @position field"));
            }
        }
        _ if *result != Type::Unit => {
            diagnostics.push(error(format!("{stage} entries must return unit")))
        }
        _ => {}
    }
    diagnostics
}

fn entry_parameters(header: &[Token]) -> Vec<Parameter<'_>> {
    let Some(open) = header.iter().position(|token| punctuation(token, '(')) else {
        return Vec::new();
    };
    let Some(close) = matching(header, open, '(', ')') else {
        return Vec::new();
    };
    split_ranges(header, open + 1, close)
        .into_iter()
        .filter_map(|(start, _end)| {
            let kind = atom(header.get(start)?)?;
            Some(Parameter {
                kind,
                span: header[start].span,
            })
        })
        .collect()
}

fn split_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut part_start = start;
    let mut depth = [0i32; 4];
    for index in start..end {
        match tokens[index].kind {
            TokenKind::Punctuation('(') => depth[0] += 1,
            TokenKind::Punctuation(')') => depth[0] -= 1,
            TokenKind::Punctuation('[') => depth[1] += 1,
            TokenKind::Punctuation(']') => depth[1] -= 1,
            TokenKind::Punctuation('<') => depth[2] += 1,
            TokenKind::Punctuation('>') => depth[2] -= 1,
            TokenKind::Punctuation('{') => depth[3] += 1,
            TokenKind::Punctuation('}') => depth[3] -= 1,
            TokenKind::Punctuation(',') if depth == [0; 4] => {
                ranges.push((part_start, index));
                part_start = index + 1;
            }
            _ => {}
        }
    }
    if part_start < end {
        ranges.push((part_start, end));
    }
    ranges
}

fn matching(tokens: &[Token], open: usize, pair: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation(character) if character == pair => depth += 1,
            TokenKind::Punctuation(character) if character == close => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_entry_stage(stage: &str) -> bool {
    matches!(
        stage,
        "compute"
            | "object"
            | "mesh"
            | "vertex"
            | "fragment"
            | "tile"
            | "raygen"
            | "intersection"
            | "anyhit"
            | "closesthit"
            | "miss"
            | "callable"
            | "node"
    )
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(V15, message)
}

fn type_error(message: impl Into<String>, span: Span) -> Diagnostic {
    error(message).with_primary_span(span)
}

fn shift(diagnostic: &mut Diagnostic, offset: usize) {
    let span = &mut diagnostic.primary_span;
    if let Some(span) = span {
        span.start += offset;
        span.end += offset;
    }
}
