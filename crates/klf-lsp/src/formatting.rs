//! `textDocument/formatting` and `rangeFormatting` (`KLF-EDITOR.md` section
//! 11.17, `KLF-FORMAT.md` section 6). The formatter answers the whole
//! document; the kernel turns that into edits of the whitespace that
//! changed, and keeps those touching the range for a range request.

use crate::convert;
use crate::server::Server;
use klf_ide::FormatRequest;
use klf_text::LineIndex;
use lsp_server::{ErrorCode, Request, Response};
use lsp_types::{DocumentFormattingParams, DocumentRangeFormattingParams, TextEdit};
use std::path::PathBuf;

pub(crate) const METHODS: [&str; 2] = ["textDocument/formatting", "textDocument/rangeFormatting"];

/// `KLF-LSP-9002` and the other ways a formatting request fails.
fn failed(request: &Request, message: impl Into<String>) -> Response {
    Response::new_err(
        request.id.clone(),
        ErrorCode::RequestFailed as i32,
        message.into(),
    )
}

pub(crate) fn answer(server: &Server<'_>, request: &Request) -> Response {
    let Some(service) = server.formatter() else {
        return crate::server::unhandled(request.id.clone(), &request.method);
    };
    let (uri, options, range) = if request.method == METHODS[0] {
        match serde_json::from_value::<DocumentFormattingParams>(request.params.clone()) {
            Ok(params) => (params.text_document.uri, params.options, None),
            Err(error) => return failed(request, error.to_string()),
        }
    } else {
        match serde_json::from_value::<DocumentRangeFormattingParams>(request.params.clone()) {
            Ok(params) => (params.text_document.uri, params.options, Some(params.range)),
            Err(error) => return failed(request, error.to_string()),
        }
    };
    let Some(path) = convert::path(&uri) else {
        return failed(request, "not a file");
    };
    let text = match server.document(&path) {
        Some(text) => text.to_owned(),
        None => match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return failed(request, error.to_string()),
        },
    };
    let result = service.format(&FormatRequest {
        directory: path.parent().map_or_else(PathBuf::new, PathBuf::from),
        path,
        text: text.clone(),
        width: klf_format::Options::default().width,
        tab_size: usize::try_from(options.tab_size).unwrap_or(4).max(1),
        insert_spaces: options.insert_spaces,
    });
    let formatted = match result {
        Ok(formatted) => formatted,
        Err(message) => return failed(request, message),
    };
    let encoding = server.agreement.encoding;
    let index = LineIndex::new(&text);
    let mut edits = klf_format::edits(&text, &formatted);
    if let Some(range) = range {
        let start = convert::offset(&index, range.start, encoding) as usize;
        let end = convert::offset(&index, range.end, encoding) as usize;
        edits = klf_format::restrict(edits, start, end);
    }
    let edits: Vec<TextEdit> = edits
        .into_iter()
        .map(|edit| {
            let start = u32::try_from(edit.start).unwrap_or(u32::MAX);
            let end = u32::try_from(edit.end).unwrap_or(u32::MAX);
            TextEdit::new(convert::range(&index, start, end, encoding), edit.text)
        })
        .collect();
    crate::handlers::reply(request, edits)
}
