use tir_diag::Span;
use tir_format::{BodyParseError, FunctionBody, parse_tvisa_function_body};

use super::{TextItem, TextItemKind};

/// Parsed TVISA body for one function item.
///
/// The item span is absolute in the image source. Body and parse-error spans
/// remain relative to that item's text, matching `parse_tvisa_function_body`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextFunctionBody {
    pub span: Span,
    pub body: Option<FunctionBody>,
    pub error: Option<BodyParseError>,
}

pub(super) fn parse_all(source: &str, items: &[TextItem]) -> Vec<TextFunctionBody> {
    items
        .iter()
        .filter(|item| item.kind == TextItemKind::Function)
        .map(|item| {
            let text = source
                .get(item.span.start..item.span.end)
                .unwrap_or_default();
            match parse_tvisa_function_body(text) {
                Ok(body) => TextFunctionBody {
                    span: item.span,
                    body,
                    error: None,
                },
                Err(error) => TextFunctionBody {
                    span: item.span,
                    body: None,
                    error: Some(error),
                },
            }
        })
        .collect()
}
