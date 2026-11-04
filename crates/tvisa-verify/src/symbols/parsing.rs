use tir_diag::Diagnostic;
use tir_format::text::lex;
use tvisa_format::{TextCallingConvention, TextImage, TextImport, TextImportPayload, TextItem};

use super::{
    ENCODING, FunctionRecord, VFY11, atom, forbidden_tag, has_attribute, is_atom, is_punctuation,
    parse_tag, read_symbol_refs, significant,
};

pub(super) fn parse_function(
    image: &TextImage,
    item: TextItem,
) -> Option<(String, FunctionRecord, Vec<Diagnostic>)> {
    let source = image.item_text(item);
    let all_tokens = lex(source).ok()?;
    let header_end = all_tokens
        .iter()
        .position(|token| is_punctuation(token, '{'))
        .unwrap_or(all_tokens.len());
    let tokens = significant(&all_tokens[..header_end]);
    if tokens.len() < 4 || !is_atom(tokens[0], "func") || !is_punctuation(tokens[2], '@') {
        return None;
    }
    let (name, _, symbol_span) = read_symbol_refs(&tokens, 2)?;
    let linkage = atom(tokens[1]).unwrap_or_default();
    let address_taken = has_attribute(&tokens, "address_taken");
    let signature_name = tokens.windows(3).find_map(|window| {
        (is_atom(window[0], "sig") && is_punctuation(window[1], '$'))
            .then(|| atom(window[2]))
            .flatten()
    });
    let is_gpu = image
        .signatures
        .iter()
        .find(|signature| Some(signature.name.as_str()) == signature_name)
        .is_some_and(|signature| {
            matches!(
                signature.convention,
                TextCallingConvention::Gpu | TextCallingConvention::Entry
            )
        });
    let mut diagnostics = Vec::new();
    let tag = parse_tag(&tokens, &mut diagnostics);
    if !matches!(linkage, "local" | "image" | "exported") {
        diagnostics.push(
            Diagnostic::error(
                ENCODING,
                "defined functions require local, image, or exported linkage",
            )
            .with_primary_span(tokens[1].span),
        );
    }
    if is_gpu {
        if address_taken || tag.is_some() {
            diagnostics.push(
                Diagnostic::error(
                    VFY11,
                    format!("GPU function @{name} must not use CPU address_taken tags"),
                )
                .with_primary_span(symbol_span),
            );
        }
    } else {
        if address_taken && tag.is_none() {
            diagnostics.push(
                Diagnostic::error(
                    VFY11,
                    format!("address_taken function @{name} requires a tag"),
                )
                .with_primary_span(symbol_span),
            );
        }
        if !address_taken && tag.is_some() {
            diagnostics.push(
                Diagnostic::error(
                    VFY11,
                    format!("function @{name} has a tag without address_taken"),
                )
                .with_primary_span(symbol_span),
            );
        }
    }
    if linkage == "exported" && !address_taken {
        diagnostics.push(
            Diagnostic::error(
                VFY11,
                format!("exported function @{name} must be address_taken"),
            )
            .with_primary_span(symbol_span),
        );
    }
    if let Some(tag) = tag
        && forbidden_tag(tag)
    {
        diagnostics.push(
            Diagnostic::error(
                VFY11,
                format!("function @{name} has forbidden type tag 0x{tag:08X}"),
            )
            .with_primary_span(symbol_span),
        );
    }
    Some((
        name,
        FunctionRecord {
            address_taken: address_taken && !is_gpu,
            imported: false,
            c_abi: signature_name.is_some_and(|name| {
                image.signatures.iter().any(|signature| {
                    signature.name == name && signature.convention == TextCallingConvention::C
                })
            }),
        },
        diagnostics,
    ))
}

pub(super) fn parse_function_import(
    image: &TextImage,
    import: &TextImport,
) -> Option<(String, FunctionRecord, Vec<Diagnostic>)> {
    let TextImportPayload::Function { signature, tag, .. } = &import.payload else {
        return None;
    };
    let name = import.symbol.clone();
    let mut diagnostics = Vec::new();
    if forbidden_tag(*tag) {
        diagnostics.push(
            Diagnostic::error(
                VFY11,
                format!("function import @{name} has forbidden type tag 0x{tag:08X}"),
            )
            .with_primary_span(import.symbol_span),
        );
    }
    Some((
        name,
        FunctionRecord {
            address_taken: true,
            imported: true,
            c_abi: image
                .signatures
                .iter()
                .any(|item| item.name == *signature && item.convention == TextCallingConvention::C),
        },
        diagnostics,
    ))
}
