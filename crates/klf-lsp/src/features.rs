//! The requests that explain and complete code (`KLF-EDITOR.md` sections
//! 11.1, 11.2, 11.5 to 11.7, 11.12 to 11.15 and 11.18).

use crate::convert;
use crate::handlers::{cursor, reply, request_path};
use crate::server::Server;
use klf_diag::Severity;
use klf_facts::{FoldKind, HintKind};
use klf_ide::{CompletionKind, LinkDestination, Snapshot};
use klf_text::LineIndex;
use lsp_server::{Request, Response};
use lsp_types::{
    CodeLens, Command, CompletionItem, CompletionItemTag, CompletionTextEdit, DocumentLink,
    Documentation, FoldingRange, FoldingRangeKind, Hover, HoverContents, InlayHint, InlayHintKind,
    InlayHintLabel, MarkupContent, MarkupKind, NumberOrString, ParameterInformation,
    ParameterLabel, Range, SelectionRange, SemanticTokens, SignatureHelp, SignatureInformation,
    TextEdit, Uri,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;

/// Answers the requests of this module; `None` for any other.
pub(crate) fn answer(
    server: &Server<'_>,
    snapshot: &Snapshot,
    request: &Request,
) -> Option<Response> {
    let path = request_path(request)?;
    let index = snapshot.line_index(&path)?;
    let cx = Context {
        server,
        snapshot,
        request,
        encoding: server.agreement.encoding,
        path,
        index,
    };
    match request.method.as_str() {
        "textDocument/semanticTokens/full" | "textDocument/semanticTokens/full/delta" => {
            semantic_tokens(&cx)
        }
        "textDocument/semanticTokens/range" => semantic_tokens_range(&cx),
        "textDocument/hover" => hover(&cx),
        "textDocument/completion" => completion(&cx),
        "textDocument/signatureHelp" => signature_help(&cx),
        "textDocument/inlayHint" => inlay_hints(&cx),
        "textDocument/foldingRange" => folding_ranges(&cx),
        "textDocument/selectionRange" => selection_ranges(&cx),
        "textDocument/documentLink" => document_links(&cx),
        "textDocument/codeLens" => code_lenses(&cx),
        "textDocument/diagnostic" => pull_diagnostics(&cx),
        "klf/runnables" => runnables(&cx),
        "klf/fileInfo" => file_info(&cx),
        "klf/kir" => kir(&cx),
        "klf/expand" => expand(&cx),
        _ => None,
    }
}

/// Every handler returns an `Option` so `answer` can dispatch to all of them
/// alike.
struct Context<'a> {
    server: &'a Server<'a>,
    snapshot: &'a Snapshot,
    request: &'a Request,
    encoding: klf_text::Encoding,
    path: PathBuf,
    index: Rc<LineIndex>,
}

#[allow(clippy::unnecessary_wraps)]
fn semantic_tokens(cx: &Context<'_>) -> Option<Response> {
    let tokens = cx.snapshot.tokens(&cx.path);
    let data = convert::encode_tokens(
        &tokens,
        &cx.index,
        cx.encoding,
        cx.server.agreement.multiline_tokens,
    );
    Some(reply(
        cx.request,
        SemanticTokens {
            result_id: None,
            data,
        },
    ))
}

#[allow(clippy::unnecessary_wraps)]
fn semantic_tokens_range(cx: &Context<'_>) -> Option<Response> {
    let range: Range = serde_json::from_value(cx.request.params.get("range")?.clone()).ok()?;
    let (start, end) = (
        convert::offset(&cx.index, range.start, cx.encoding),
        convert::offset(&cx.index, range.end, cx.encoding),
    );
    let mut tokens = cx.snapshot.tokens(&cx.path);
    tokens.retain(|token| token.end > start && token.start < end);
    let data = convert::encode_tokens(
        &tokens,
        &cx.index,
        cx.encoding,
        cx.server.agreement.multiline_tokens,
    );
    Some(reply(
        cx.request,
        SemanticTokens {
            result_id: None,
            data,
        },
    ))
}

#[allow(clippy::unnecessary_wraps)]
fn hover(cx: &Context<'_>) -> Option<Response> {
    let hover = cursor(cx.snapshot, cx.server, cx.request)
        .and_then(|(_, offset)| cx.snapshot.hover(&cx.path, offset));
    Some(reply(
        cx.request,
        hover.map(|hover| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover.text,
            }),
            range: Some(convert::range(
                &cx.index,
                hover.span.start,
                hover.span.end,
                cx.encoding,
            )),
        }),
    ))
}

#[allow(clippy::unnecessary_wraps)]
fn completion(cx: &Context<'_>) -> Option<Response> {
    let (_, offset) = cursor(cx.snapshot, cx.server, cx.request)?;
    let (items, replaced) = cx.snapshot.completions(&cx.path, offset);
    let range = convert::range(&cx.index, replaced.start, replaced.end, cx.encoding);
    let items: Vec<CompletionItem> = items
        .into_iter()
        .map(|item| CompletionItem {
            kind: Some(match item.kind {
                CompletionKind::Symbol(kind) => convert::completion_kind(kind),
                CompletionKind::Keyword => lsp_types::CompletionItemKind::KEYWORD,
            }),
            detail: (!item.detail.is_empty()).then(|| item.detail.clone()),
            documentation: (!item.documentation.is_empty()).then(|| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: item.documentation.clone(),
                })
            }),
            tags: item.deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                item.label.clone(),
            ))),
            label: item.label,
            ..CompletionItem::default()
        })
        .collect();
    Some(reply(cx.request, items))
}

#[allow(clippy::unnecessary_wraps)]
fn signature_help(cx: &Context<'_>) -> Option<Response> {
    let help = cursor(cx.snapshot, cx.server, cx.request)
        .and_then(|(_, offset)| cx.snapshot.signature_help(&cx.path, offset));
    Some(reply(
        cx.request,
        help.map(|help| SignatureHelp {
            signatures: vec![SignatureInformation {
                label: help.label,
                documentation: (!help.documentation.is_empty()).then_some({
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: help.documentation,
                    })
                }),
                parameters: Some(
                    help.parameters
                        .into_iter()
                        .map(|label| ParameterInformation {
                            label: ParameterLabel::Simple(label),
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: Some(help.active),
            }],
            active_signature: Some(0),
            active_parameter: Some(help.active),
        }),
    ))
}

#[allow(clippy::unnecessary_wraps)]
fn inlay_hints(cx: &Context<'_>) -> Option<Response> {
    let range: Range = serde_json::from_value(cx.request.params.get("range")?.clone()).ok()?;
    let (start, end) = (
        convert::offset(&cx.index, range.start, cx.encoding),
        convert::offset(&cx.index, range.end, cx.encoding),
    );
    let hints: Vec<InlayHint> = cx
        .snapshot
        .inlay_hints(&cx.path, start, end)
        .into_iter()
        .map(|hint| {
            let position = convert::position(&cx.index, hint.at, cx.encoding);
            InlayHint {
                position,
                label: InlayHintLabel::String(hint.label),
                kind: match hint.kind {
                    HintKind::Type => Some(InlayHintKind::TYPE),
                    HintKind::Parameter => Some(InlayHintKind::PARAMETER),
                    _ => None,
                },
                text_edits: hint
                    .insert
                    .map(|text| vec![TextEdit::new(Range::new(position, position), text)]),
                tooltip: None,
                padding_left: Some(hint.padding_left),
                padding_right: Some(hint.padding_right),
                data: None,
            }
        })
        .collect();
    Some(reply(cx.request, hints))
}

#[allow(clippy::unnecessary_wraps)]
fn folding_ranges(cx: &Context<'_>) -> Option<Response> {
    let folds: Vec<FoldingRange> = cx
        .snapshot
        .folds(&cx.path)
        .into_iter()
        .filter_map(|(start, end, kind)| {
            let (from, to) = (cx.index.line_of(start), cx.index.line_of(end));
            (to > from).then_some(FoldingRange {
                start_line: from,
                start_character: None,
                end_line: to,
                end_character: None,
                kind: match kind {
                    FoldKind::Comment => Some(FoldingRangeKind::Comment),
                    FoldKind::Imports => Some(FoldingRangeKind::Imports),
                    FoldKind::Region => Some(FoldingRangeKind::Region),
                    FoldKind::Other => None,
                },
                collapsed_text: None,
            })
        })
        .collect();
    Some(reply(cx.request, folds))
}

#[allow(clippy::unnecessary_wraps)]
fn selection_ranges(cx: &Context<'_>) -> Option<Response> {
    let positions: Vec<lsp_types::Position> =
        serde_json::from_value(cx.request.params.get("positions")?.clone()).ok()?;
    let ranges: Vec<SelectionRange> = positions
        .into_iter()
        .map(|position| {
            let offset = convert::offset(&cx.index, position, cx.encoding);
            let mut chain: Option<SelectionRange> = None;
            for (start, end) in cx.snapshot.selection(&cx.path, offset).into_iter().rev() {
                chain = Some(SelectionRange {
                    range: convert::range(&cx.index, start, end, cx.encoding),
                    parent: chain.map(Box::new),
                });
            }
            chain.unwrap_or(SelectionRange {
                range: Range::new(position, position),
                parent: None,
            })
        })
        .collect();
    Some(reply(cx.request, ranges))
}

#[allow(clippy::unnecessary_wraps)]
fn document_links(cx: &Context<'_>) -> Option<Response> {
    let links: Vec<DocumentLink> = cx
        .snapshot
        .links(&cx.path)
        .into_iter()
        .filter_map(|(start, end, destination)| {
            let target = match destination {
                LinkDestination::File(target) => convert::uri(&target)?,
                LinkDestination::Url(url) => Uri::from_str(&url).ok()?,
                LinkDestination::Span(span) => {
                    let line = cx.snapshot.line_index(&span.path)?.line_of(span.start) + 1;
                    Uri::from_str(&format!("{}#L{line}", convert::uri(&span.path)?.as_str()))
                        .ok()?
                }
            };
            Some(DocumentLink {
                range: convert::range(&cx.index, start, end, cx.encoding),
                target: Some(target),
                tooltip: None,
                data: None,
            })
        })
        .collect();
    Some(reply(cx.request, links))
}

#[allow(clippy::unnecessary_wraps)]
fn code_lenses(cx: &Context<'_>) -> Option<Response> {
    let lenses: Vec<CodeLens> = cx
        .snapshot
        .runnables(&cx.path)
        .into_iter()
        .map(|runnable| {
            let range = convert::range(&cx.index, runnable.start, runnable.start, cx.encoding);
            // A shader's lens opens its output views (section 14.5); every
            // other runnable runs.
            if runnable.kind == klf_facts::RunnableKind::Shader {
                let shader = runnable.arguments.first().cloned().unwrap_or_default();
                return CodeLens {
                    range,
                    command: Some(Command {
                        title: format!("▤ {shader}"),
                        command: "klf.showOutput".to_owned(),
                        arguments: Some(vec![serde_json::json!({
                            "textDocument": { "uri": convert::uri(&cx.path) },
                            "shader": shader,
                            "targets": runnable.arguments.get(1).cloned().unwrap_or_default(),
                        })]),
                    }),
                    data: None,
                };
            }
            CodeLens {
                range,
                command: Some(Command {
                    title: format!("▶ {}", runnable.label),
                    command: "klf.run".to_owned(),
                    arguments: Some(vec![serde_json::json!({
                        "driver": cx.server.source(),
                        "kind": runnable.kind.name(),
                        "arguments": runnable.arguments,
                    })]),
                }),
                data: None,
            }
        })
        .collect();
    Some(reply(cx.request, lenses))
}

#[allow(clippy::unnecessary_wraps)]
fn pull_diagnostics(cx: &Context<'_>) -> Option<Response> {
    let items = diagnostics(cx.server, cx.snapshot, &cx.path);
    Some(reply(
        cx.request,
        serde_json::json!({ "kind": "full", "items": items }),
    ))
}

pub(crate) fn diagnostics(
    server: &Server<'_>,
    snapshot: &Snapshot,
    path: &Path,
) -> Vec<lsp_types::Diagnostic> {
    let Some(index) = snapshot.line_index(path) else {
        return Vec::new();
    };
    snapshot
        .diagnostics(path)
        .into_iter()
        .map(|diagnostic| lsp_types::Diagnostic {
            range: convert::range(
                &index,
                diagnostic.start,
                diagnostic.end,
                server.agreement.encoding,
            ),
            severity: Some(match diagnostic.severity {
                Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
                Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                Severity::Note => lsp_types::DiagnosticSeverity::INFORMATION,
                Severity::Remark => lsp_types::DiagnosticSeverity::HINT,
            }),
            code: Some(NumberOrString::String(diagnostic.code.clone())),
            source: Some(
                diagnostic
                    .source
                    .clone()
                    .unwrap_or_else(|| server.source().to_owned()),
            ),
            message: diagnostic.message.clone(),
            ..lsp_types::Diagnostic::default()
        })
        .collect()
}

/// The project directory of the request's document, which runnables run in.
fn project_root(cx: &Context<'_>) -> Option<PathBuf> {
    let uri = convert::uri(&cx.path)?;
    cx.server
        .project_naming(uri.as_str())
        .map(|project| project.root)
}

/// `klf/runnables` (section 17.2): every runnable of the document, or the
/// ones whose range holds `position`.
#[allow(clippy::unnecessary_wraps)]
fn runnables(cx: &Context<'_>) -> Option<Response> {
    let at = cx
        .request
        .params
        .get("position")
        .and_then(|position| serde_json::from_value(position.clone()).ok())
        .map(|position| convert::offset(&cx.index, position, cx.encoding));
    let cwd = project_root(cx).map(|root| root.to_string_lossy().into_owned());
    let found: Vec<serde_json::Value> = cx
        .snapshot
        .runnables(&cx.path)
        .into_iter()
        .filter(|runnable| at.is_none_or(|at| runnable.start <= at && at <= runnable.end))
        .map(|runnable| {
            serde_json::json!({
                "label": runnable.label,
                "kind": runnable.kind.name(),
                "arguments": runnable.arguments,
                "cwd": cwd,
                "range": convert::range(&cx.index, runnable.start, runnable.end, cx.encoding),
            })
        })
        .collect();
    Some(reply(cx.request, found))
}

/// `klf/fileInfo` (sections 13.5 and 17.2): the mode the document compiled
/// in. Fields the analysis did not record are null.
#[allow(clippy::unnecessary_wraps)]
fn file_info(cx: &Context<'_>) -> Option<Response> {
    let analysis = &cx.snapshot.analysis;
    let info = analysis
        .file(&cx.path)
        .and_then(|file| analysis.file_info.iter().find(|info| info.file == file));
    let text = |value: Option<&String>| value.filter(|value| !value.is_empty()).cloned();
    Some(reply(
        cx.request,
        serde_json::json!({
            "project": project_root(cx),
            "fileMode": text(info.map(|info| &info.mode)),
            "memory": text(info.map(|info| &info.memory)),
            "convention": text(info.map(|info| &info.convention)),
            "unit": info.and_then(|info| info.unit),
        }),
    ))
}

/// `klf/kir` (section 13.7): a view of the KIR of the function at the
/// position, raw or canonical. The function is the last one the document
/// names at or before the position, as KIR keeps only where a function was
/// named, not where its body ends.
#[allow(clippy::unnecessary_wraps)]
fn kir(cx: &Context<'_>) -> Option<Response> {
    let Some((path, offset)) = cursor(cx.snapshot, cx.server, cx.request) else {
        return Some(reply(cx.request, serde_json::Value::Null));
    };
    let canonical = cx
        .request
        .params
        .get("canonical")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let analysis = &cx.snapshot.analysis;
    let function = analysis.file(&path).and_then(|file| {
        analysis
            .kir
            .iter()
            .filter(|function| function.file == file && function.start <= offset)
            .max_by_key(|function| function.start)
    });
    let opened = function.and_then(|function| {
        let uri = convert::uri(&path)?;
        let project = cx.server.project_naming(uri.as_str())?;
        let stage = if canonical { "canonical" } else { "raw" };
        let source = crate::views::Source::Kir {
            file: path.clone(),
            function: function.name.clone(),
            canonical,
        };
        let name = format!("{}.{stage}.kir", function.name);
        cx.server.views.open(&project, &name, source, analysis)
    });
    Some(reply(
        cx.request,
        opened.map(|(uri, _)| serde_json::json!({ "uri": uri })),
    ))
}

/// `klf/expand` (section 13.6): a view of what the innermost macro
/// invocation holding the position expanded to, with the macro's name.
#[allow(clippy::unnecessary_wraps)]
fn expand(cx: &Context<'_>) -> Option<Response> {
    let Some((path, offset)) = cursor(cx.snapshot, cx.server, cx.request) else {
        return Some(reply(cx.request, serde_json::Value::Null));
    };
    let analysis = &cx.snapshot.analysis;
    let expansion = analysis.file(&path).and_then(|file| {
        analysis
            .expansions
            .iter()
            .filter(|expansion| {
                expansion.origin == file && expansion.start <= offset && offset <= expansion.end
            })
            .min_by_key(|expansion| expansion.end - expansion.start)
    });
    let opened = expansion.and_then(|expansion| {
        let uri = convert::uri(&path)?;
        let project = cx.server.project_naming(uri.as_str())?;
        let source = crate::views::Source::Expansion {
            origin: path.clone(),
            start: expansion.start,
            name: expansion.name.clone(),
            language_id: cx.server.source().to_owned(),
        };
        let view = format!("{}@{}.expansion", expansion.name, expansion.start);
        let (uri, _) = cx.server.views.open(&project, &view, source, analysis)?;
        Some(serde_json::json!({ "uri": uri, "name": expansion.name }))
    });
    Some(reply(cx.request, opened))
}
