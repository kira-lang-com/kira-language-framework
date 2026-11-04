use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{TokenKind, lex};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind};

const VFY22: Code = Code::new(Dialect::Tvisa, Area::Verifier, 22);

pub fn verify_image_metadata(image: &TextImage) -> Vec<Diagnostic> {
    let Some(metadata_span) = metadata_span(image) else {
        return Vec::new();
    };
    let mut functions = HashMap::new();
    for (index, item) in image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Function)
        .enumerate()
    {
        let source = image.item_text(*item);
        let (Some(name), Some(signature_name)) = (
            super::super::gpu::function_symbol(source),
            super::super::gpu::function_signature_name(source),
        ) else {
            continue;
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|signature| signature.name == signature_name)
        else {
            continue;
        };
        functions.insert(name, (index, signature.convention, item.span));
    }

    let mut diagnostics = Vec::new();
    if let Some(entry) = &image.image_metadata.entry_function {
        verify_cpu_function(entry, "entry", metadata_span, &functions, &mut diagnostics);
    }
    verify_function_list(
        &image.image_metadata.initializers,
        "initializer",
        metadata_span,
        &functions,
        &mut diagnostics,
    );
    verify_function_list(
        &image.image_metadata.finalizers,
        "finalizer",
        metadata_span,
        &functions,
        &mut diagnostics,
    );

    let mut capability_union = HashSet::new();
    for item in image
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Entry)
    {
        if let Some(entry) = image.gpu_entry_for_item(*item)
            && entry
                .kind
                .as_deref()
                .is_some_and(|stage| stage != "visible")
            && let Some(capabilities) = super::fields::declared_capabilities(entry)
        {
            capability_union.extend(capabilities);
        }
    }
    let recorded: HashSet<_> = image
        .image_metadata
        .gpu_capabilities
        .iter()
        .cloned()
        .collect();
    if capability_union != recorded {
        diagnostics.push(
            Diagnostic::error(
                VFY22,
                "metadata image gpu_capabilities does not match the GPU entry union",
            )
            .with_primary_span(metadata_span),
        );
    }
    diagnostics
}

fn verify_function_list(
    records: &[tvisa_format::TextImageFunction],
    label: &str,
    span: Span,
    functions: &HashMap<String, (usize, TextCallingConvention, Span)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut previous = None;
    for record in records {
        let Some(index) =
            verify_cpu_function(&record.function, label, span, functions, diagnostics)
        else {
            continue;
        };
        let key = (record.priority, index);
        if previous.is_some_and(|previous| previous >= key) {
            diagnostics.push(
                Diagnostic::error(
                    VFY22,
                    format!("metadata image {label} records are not ordered by priority and function index"),
                )
                .with_primary_span(span),
            );
        }
        previous = Some(key);
    }
}

fn verify_cpu_function(
    symbol: &str,
    label: &str,
    span: Span,
    functions: &HashMap<String, (usize, TextCallingConvention, Span)>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let name = symbol.strip_prefix('@').unwrap_or(symbol);
    let Some((index, convention, function_span)) = functions.get(name).copied() else {
        diagnostics.push(
            Diagnostic::error(
                VFY22,
                format!("metadata image {label} @{name} is not a defined function"),
            )
            .with_primary_span(span),
        );
        return None;
    };
    if matches!(
        convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    ) {
        diagnostics.push(
            Diagnostic::error(
                VFY22,
                format!("metadata image {label} @{name} must be a CPU function"),
            )
            .with_primary_span(span)
            .with_note_at(function_span, "referenced function is declared here"),
        );
        return None;
    }
    Some(index)
}

fn metadata_span(image: &TextImage) -> Option<Span> {
    image.items.iter().find_map(|item| {
        if item.kind != TextItemKind::Metadata {
            return None;
        }
        let source = image.item_text(*item);
        let tokens = lex(source).ok()?;
        let mut words = tokens.iter().filter_map(|token| match &token.kind {
            TokenKind::Atom(value) => Some(value.as_str()),
            _ => None,
        });
        (words.next() == Some("metadata") && words.next() == Some("image")).then_some(item.span)
    })
}
