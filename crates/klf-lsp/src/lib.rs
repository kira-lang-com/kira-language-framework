//! The KLF language server (`KLF-EDITOR.md` sections 10 and 11): the
//! protocol over an analysis host. A generated driver serves it as
//! `<driver> lsp`; `klf lsp` serves the kernel's own languages.

mod convert;
mod features;
mod formatting;
mod handlers;
mod schedule;
mod server;
mod views;

pub use server::ServerConfig;

/// The semantic token types and modifiers every KLF server announces, in
/// legend order, for packages that declare them to their editor.
#[must_use]
pub fn legend_names() -> (Vec<String>, Vec<String>) {
    let legend = convert::legend();
    (
        legend
            .token_types
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect(),
        legend
            .token_modifiers
            .iter()
            .map(|modifier| modifier.as_str().to_owned())
            .collect(),
    )
}

use klf_text::Encoding;
use lsp_server::Connection;
use lsp_types::{
    CallHierarchyServerCapability, CodeLensOptions, CompletionOptions, DocumentLinkOptions,
    FoldingRangeProviderCapability, HoverProviderCapability, ImplementationProviderCapability,
    InitializeParams, InlayHintServerCapabilities, OneOf, PositionEncodingKind, RenameOptions,
    SelectionRangeProviderCapability, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensServerCapabilities, ServerCapabilities, SignatureHelpOptions,
    TextDocumentSyncCapability, TextDocumentSyncKind, TypeDefinitionProviderCapability,
    WorkDoneProgressOptions,
};
use std::path::PathBuf;

/// Serves over standard input and output until the client exits.
///
/// # Errors
/// Returns a message when the protocol breaks down.
pub fn serve(config: ServerConfig) -> Result<(), String> {
    let (connection, threads) = Connection::stdio();
    run(&connection, config)?;
    drop(connection);
    threads.join().map_err(|error| error.to_string())
}

/// Serves over `connection`: initialization, then the server loop.
///
/// # Errors
/// Returns a message when the protocol breaks down.
pub fn run(connection: &Connection, config: ServerConfig) -> Result<(), String> {
    let (id, params) = connection
        .initialize_start()
        .map_err(|error| error.to_string())?;
    let params: InitializeParams =
        serde_json::from_value(params).map_err(|error| error.to_string())?;
    let agreement = agree(&params);
    let result = serde_json::json!({
        "capabilities": capabilities(&config, agreement.encoding),
        "serverInfo": { "name": format!("{} lsp", config.name), "version": config.version },
        "klf": { "version": env!("CARGO_PKG_VERSION"), "features": features() },
    });
    connection
        .initialize_finish(id, result)
        .map_err(|error| error.to_string())?;
    server::Server::new(connection, config, agreement).run()
}

/// The `klf/` extensions this server answers (section 17.1).
fn features() -> Vec<&'static str> {
    vec![
        "klf/virtualFile",
        "klf/shaderOutput",
        "klf/shaderOutputChanged",
        "klf/status",
        "klf/fileInfo",
        "klf/runnables",
        "klf/reloadProjects",
        "klf/kir",
        "klf/expand",
    ]
}

fn agree(params: &InitializeParams) -> server::Agreement {
    let offered: Vec<Encoding> = params
        .capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_ref())
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(|kind| Encoding::from_name(kind.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let multiline_tokens = params
        .capabilities
        .text_document
        .as_ref()
        .and_then(|document| document.semantic_tokens.as_ref())
        .and_then(|tokens| tokens.multiline_token_support)
        .unwrap_or(false);
    let mut folders: Vec<PathBuf> = params
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|folder| convert::path(&folder.uri))
        .collect();
    #[allow(deprecated)]
    if folders.is_empty()
        && let Some(root) = params.root_uri.as_ref().and_then(convert::path)
    {
        folders.push(root);
    }
    server::Agreement {
        encoding: Encoding::negotiate(&offered),
        multiline_tokens,
        folders,
    }
}

fn capabilities(config: &ServerConfig, encoding: Encoding) -> ServerCapabilities {
    let triggers = config.languages.access_operators();
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::from(encoding.name().to_owned())),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions::default(),
                legend: convert::legend(),
                range: Some(true),
                full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
            },
        )),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(lsp_types::DeclarationCapability::Simple(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(triggers),
            ..CompletionOptions::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            retrigger_characters: None,
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
            lsp_types::InlayHintOptions::default(),
        ))),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        document_formatting_provider: config.formatter.as_ref().map(|_| OneOf::Left(true)),
        document_range_formatting_provider: config.formatter.as_ref().map(|_| OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}
