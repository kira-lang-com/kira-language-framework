use crate::{ModuleHeader, SemanticsMode, TextItemKind, TextParseError};
use std::fmt::Write as _;
use tir_diag::Span;
use tir_target::Platform;

pub(super) fn profile_name(profile: tir_target::Profile) -> &'static str {
    match profile {
        tir_target::Profile::App => "app",
        tir_target::Profile::System => "system",
        tir_target::Profile::Driver => "driver",
        tir_target::Profile::Kernel => "kernel",
    }
}

pub(super) fn platform_name(platform: tir_target::Platform) -> &'static str {
    match platform {
        Platform::Portable => "portable",
        Platform::Tessera => "tessera",
        Platform::Macos => "macos",
        Platform::Ios => "ios",
        Platform::Windows => "windows",
        Platform::Linux => "linux",
        Platform::Android => "android",
        Platform::Web => "web",
        Platform::Wasi => "wasi",
    }
}

pub(super) fn semantics_name(semantics: SemanticsMode) -> &'static str {
    match semantics {
        SemanticsMode::Defined => "defined",
        SemanticsMode::Permissive => "permissive",
    }
}

pub(super) fn format_uuid(uuid: [u8; 16]) -> String {
    let mut text = String::with_capacity(36);
    for (index, byte) in uuid.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    text
}

pub(super) fn parse_source_hash(header: &ModuleHeader) -> Result<Option<[u8; 32]>, TextParseError> {
    let Some(source_hash) = &header.source_hash else {
        return if header.profile == tir_target::Profile::Driver {
            Err(error(
                Span::new(0, 0),
                "driver module header is missing its source hash",
            ))
        } else {
            Ok(None)
        };
    };
    if header.profile != tir_target::Profile::Driver {
        return Err(error(
            Span::new(0, 0),
            "source hash is only valid for driver modules",
        ));
    }
    if source_hash.len() != 64
        || !source_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(
            Span::new(0, 0),
            "driver source hash must contain 64 lowercase hexadecimal digits",
        ));
    }

    let mut bytes = [0; 32];
    for (index, pair) in source_hash.as_bytes().chunks_exact(2).enumerate() {
        let value = std::str::from_utf8(pair)
            .ok()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .ok_or_else(|| error(Span::new(0, 0), "invalid driver source hash"))?;
        bytes[index] = value;
    }
    Ok(Some(bytes))
}

pub(super) fn item_name(kind: TextItemKind) -> &'static str {
    match kind {
        TextItemKind::Import => "import",
        TextItemKind::Type => "type",
        TextItemKind::Global => "global",
        TextItemKind::Function => "function",
        TextItemKind::Entry => "entry",
        TextItemKind::Variants => "variants",
        TextItemKind::Trait => "trait",
        TextItemKind::Conformance => "conformance",
        TextItemKind::VTable => "vtable",
    }
}

pub(super) fn error(span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span,
        message: message.into(),
    }
}
