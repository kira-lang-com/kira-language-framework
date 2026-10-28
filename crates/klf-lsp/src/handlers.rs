//! Request dispatch and the navigation requests (`KLF-EDITOR.md` sections
//! 11.3, 11.4, 11.8, 11.9, 11.11 and 11.16).

use crate::convert;
use crate::features;
use crate::server::{Server, unhandled};
use klf_facts::Entity;
use klf_ide::{Access, FileSpan, Snapshot, SymbolItem};
use lsp_server::{Request, Response};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, DocumentHighlight,
    DocumentHighlightKind, DocumentSymbol, GotoDefinitionResponse, Location, Position,
    PrepareRenameResponse, SymbolInformation, TextEdit, TypeHierarchyItem, Uri, WorkspaceEdit,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The file a request is about: its text document, or the document of the
/// hierarchy item it continues.
pub(crate) fn request_path(request: &Request) -> Option<PathBuf> {
    let params = &request.params;
    let uri = params
        .get("textDocument")
        .or_else(|| params.get("item"))
        .and_then(|value| value.get("uri"))?;
    convert::path(&serde_json::from_value::<Uri>(uri.clone()).ok()?)
}

pub(crate) fn reply(request: &Request, result: impl serde::Serialize) -> Response {
    Response::new_ok(request.id.clone(), result)
}

/// The cursor of a request that has one: its file and byte offset.
pub(crate) fn cursor(
    snapshot: &Snapshot,
    server: &Server<'_>,
    request: &Request,
) -> Option<(PathBuf, u32)> {
    let path = request_path(request)?;
    let position: Position =
        serde_json::from_value(request.params.get("position")?.clone()).ok()?;
    let index = snapshot.line_index(&path)?;
    Some((
        path,
        convert::offset(&index, position, server.agreement.encoding),
    ))
}

pub(crate) fn answer(
    server: &Server<'_>,
    snapshot: Option<&Snapshot>,
    request: &Request,
) -> Response {
    let Some(snapshot) = snapshot else {
        return unhandled(request.id.clone(), &request.method);
    };
    if let Some(response) = features::answer(server, snapshot, request) {
        return response;
    }
    let encoding = server.agreement.encoding;
    let at = || cursor(snapshot, server, request);
    let locations = |spans: Vec<FileSpan>| -> Vec<Location> {
        spans
            .iter()
            .filter_map(|span| convert::location(snapshot, span, encoding))
            .collect()
    };
    match request.method.as_str() {
        "textDocument/definition"
        | "textDocument/declaration"
        | "textDocument/typeDefinition"
        | "textDocument/implementation" => {
            let spans =
                at().map_or_else(Vec::new, |(path, offset)| match request.method.as_str() {
                    "textDocument/definition" => snapshot.definition(&path, offset),
                    "textDocument/declaration" => snapshot.declaration(&path, offset),
                    "textDocument/typeDefinition" => snapshot.type_definition(&path, offset),
                    _ => snapshot.implementations(&path, offset),
                });
            reply(request, GotoDefinitionResponse::Array(locations(spans)))
        }
        "textDocument/references" => {
            let declaration = request
                .params
                .pointer("/context/includeDeclaration")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let spans = at().map_or_else(Vec::new, |(path, offset)| {
                snapshot.references(&path, offset, declaration)
            });
            reply(request, locations(spans))
        }
        "textDocument/documentHighlight" => highlights(snapshot, request, at(), encoding),
        "textDocument/prepareRename" => {
            let prepared = at().and_then(|(path, offset)| snapshot.prepare_rename(&path, offset));
            let response = prepared.and_then(|(span, placeholder)| {
                Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: convert::location(snapshot, &span, encoding)?.range,
                    placeholder,
                })
            });
            reply(request, response)
        }
        "textDocument/rename" => rename(snapshot, request, at(), encoding),
        "textDocument/documentSymbol" => {
            let symbols: Vec<DocumentSymbol> = request_path(request)
                .map(|path| snapshot.document_symbols(&path))
                .unwrap_or_default()
                .iter()
                .filter_map(|item| document_symbol(snapshot, item, encoding))
                .collect();
            reply(request, symbols)
        }
        "textDocument/prepareCallHierarchy" | "textDocument/prepareTypeHierarchy" => {
            let item = at()
                .and_then(|(path, offset)| snapshot.entity_at(&path, offset))
                .and_then(|(_, entity)| snapshot.symbol_item(entity));
            match (request.method.as_str(), item) {
                (_, None) => reply(request, Value::Null),
                ("textDocument/prepareCallHierarchy", Some(item)) => reply(
                    request,
                    call_item(snapshot, &item, encoding)
                        .into_iter()
                        .collect::<Vec<_>>(),
                ),
                (_, Some(item)) => reply(
                    request,
                    type_item(snapshot, &item, encoding)
                        .into_iter()
                        .collect::<Vec<_>>(),
                ),
            }
        }
        "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
            calls(snapshot, request, encoding)
        }
        "typeHierarchy/supertypes" | "typeHierarchy/subtypes" => {
            related_types(snapshot, request, encoding)
        }
        _ => unhandled(request.id.clone(), &request.method),
    }
}

fn highlights(
    snapshot: &Snapshot,
    request: &Request,
    at: Option<(PathBuf, u32)>,
    encoding: klf_text::Encoding,
) -> Response {
    let highlights: Vec<DocumentHighlight> = at
        .map(|(path, offset)| snapshot.highlights(&path, offset))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(span, access)| {
            Some(DocumentHighlight {
                range: convert::location(snapshot, &span, encoding)?.range,
                kind: Some(match access {
                    Access::Text => DocumentHighlightKind::TEXT,
                    Access::Read => DocumentHighlightKind::READ,
                    Access::Write => DocumentHighlightKind::WRITE,
                }),
            })
        })
        .collect();
    reply(request, highlights)
}

#[allow(clippy::mutable_key_type)]
fn rename(
    snapshot: &Snapshot,
    request: &Request,
    at: Option<(PathBuf, u32)>,
    encoding: klf_text::Encoding,
) -> Response {
    let name = request
        .params
        .get("newName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let edits = at
        .map(|(path, offset)| snapshot.rename(&path, offset, name))
        .unwrap_or_default();
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for edit in edits {
        if let Some(location) = convert::location(snapshot, &edit.span, encoding) {
            changes
                .entry(location.uri)
                .or_default()
                .push(TextEdit::new(location.range, edit.text));
        }
    }
    reply(
        request,
        WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        },
    )
}

fn calls(snapshot: &Snapshot, request: &Request, encoding: klf_text::Encoding) -> Response {
    let Some(entity) = item_entity(request) else {
        return reply(request, Value::Null);
    };
    let incoming = request.method == "callHierarchy/incomingCalls";
    let calls = if incoming {
        snapshot.incoming_calls(entity)
    } else {
        snapshot.outgoing_calls(entity)
    };
    let calls: Vec<(CallHierarchyItem, Vec<lsp_types::Range>)> = calls
        .iter()
        .filter_map(|(item, spans)| {
            let ranges = spans
                .iter()
                .filter_map(|span| Some(convert::location(snapshot, span, encoding)?.range))
                .collect();
            Some((call_item(snapshot, item, encoding)?, ranges))
        })
        .collect();
    if incoming {
        reply(
            request,
            calls
                .into_iter()
                .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
                .collect::<Vec<_>>(),
        )
    } else {
        reply(
            request,
            calls
                .into_iter()
                .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
                .collect::<Vec<_>>(),
        )
    }
}

fn related_types(snapshot: &Snapshot, request: &Request, encoding: klf_text::Encoding) -> Response {
    let Some(entity) = item_entity(request) else {
        return reply(request, Value::Null);
    };
    let related = if request.method == "typeHierarchy/supertypes" {
        snapshot.supertypes(entity)
    } else {
        snapshot.subtypes(entity)
    };
    let items: Vec<TypeHierarchyItem> = related
        .iter()
        .filter_map(|item| type_item(snapshot, item, encoding))
        .collect();
    reply(request, items)
}

/// `workspace/symbol` across every project's latest analysis.
pub(crate) fn workspace_symbols(server: &Server<'_>, request: &Request) -> Response {
    let query = request
        .params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let encoding = server.agreement.encoding;
    let mut found = Vec::new();
    for analyzed in server.analyses() {
        let snapshot = server.snapshot(analyzed);
        for item in snapshot.workspace_symbols(query, 256) {
            let Some(location) = convert::location(&snapshot, &item.selection, encoding) else {
                continue;
            };
            #[allow(deprecated)]
            found.push(SymbolInformation {
                name: item.name.clone(),
                kind: convert::symbol_kind(item.kind),
                tags: None,
                deprecated: None,
                location,
                container_name: item.container.clone(),
            });
        }
    }
    found.dedup_by(|left, right| left.name == right.name && left.location == right.location);
    reply(request, found)
}

fn document_symbol(
    snapshot: &Snapshot,
    item: &SymbolItem,
    encoding: klf_text::Encoding,
) -> Option<DocumentSymbol> {
    let index = snapshot.line_index(&item.span.path)?;
    let children: Vec<DocumentSymbol> = item
        .children
        .iter()
        .filter_map(|child| document_symbol(snapshot, child, encoding))
        .collect();
    #[allow(deprecated)]
    Some(DocumentSymbol {
        name: if item.name.is_empty() {
            "_".to_owned()
        } else {
            item.name.clone()
        },
        detail: (!item.detail.is_empty()).then(|| item.detail.clone()),
        kind: convert::symbol_kind(item.kind),
        tags: item
            .deprecated
            .then(|| vec![lsp_types::SymbolTag::DEPRECATED]),
        deprecated: None,
        range: convert::range(&index, item.span.start, item.span.end, encoding),
        selection_range: convert::range(&index, item.selection.start, item.selection.end, encoding),
        children: (!children.is_empty()).then_some(children),
    })
}

fn item_data(item: &SymbolItem) -> Value {
    serde_json::json!({ "entity": item.entity.0.to_string() })
}

fn item_entity(request: &Request) -> Option<Entity> {
    let text = request.params.pointer("/item/data/entity")?.as_str()?;
    text.parse().ok().map(Entity)
}

fn call_item(
    snapshot: &Snapshot,
    item: &SymbolItem,
    encoding: klf_text::Encoding,
) -> Option<CallHierarchyItem> {
    let (uri, range, selection_range) = item_ranges(snapshot, item, encoding)?;
    Some(CallHierarchyItem {
        name: item.name.clone(),
        kind: convert::symbol_kind(item.kind),
        tags: None,
        detail: (!item.detail.is_empty()).then(|| item.detail.clone()),
        uri,
        range,
        selection_range,
        data: Some(item_data(item)),
    })
}

fn type_item(
    snapshot: &Snapshot,
    item: &SymbolItem,
    encoding: klf_text::Encoding,
) -> Option<TypeHierarchyItem> {
    let (uri, range, selection_range) = item_ranges(snapshot, item, encoding)?;
    Some(TypeHierarchyItem {
        name: item.name.clone(),
        kind: convert::symbol_kind(item.kind),
        tags: None,
        detail: (!item.detail.is_empty()).then(|| item.detail.clone()),
        uri,
        range,
        selection_range,
        data: Some(item_data(item)),
    })
}

fn item_ranges(
    snapshot: &Snapshot,
    item: &SymbolItem,
    encoding: klf_text::Encoding,
) -> Option<(Uri, lsp_types::Range, lsp_types::Range)> {
    let index = snapshot.line_index(&item.span.path)?;
    Some((
        convert::uri(&item.span.path)?,
        convert::range(&index, item.span.start, item.span.end, encoding),
        convert::range(&index, item.selection.start, item.selection.end, encoding),
    ))
}

pub(crate) fn diagnostics(
    server: &Server<'_>,
    snapshot: &Snapshot,
    path: &Path,
) -> Vec<lsp_types::Diagnostic> {
    features::diagnostics(server, snapshot, path)
}
