use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    BinaryCodeTable, BinaryImport, BinaryImportKind, BinaryType, BinaryTypeTable, FunctionBody,
    TextItemKind, TextModule,
};
use tir_target::{Platform, Profile};

const V13: Code = Code::new(Dialect::Tir, Area::Verifier, 13);
const PLAT1: Code = Code::new(Dialect::Tir, Area::Platform, 1);

pub(super) fn verify_text_module(module: &TextModule) -> Vec<Diagnostic> {
    let platform = module.header.platform.unwrap_or(Platform::Portable);
    let mut diagnostics = Vec::new();
    if platform == Platform::Portable
        && !matches!(module.header.profile, Profile::App | Profile::System)
    {
        diagnostics.push(Diagnostic::error(
            PLAT1,
            "portable modules must use the app or system profile",
        ));
    }

    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Import)
    {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else { continue };
        let ridl_name = ridl_import_name(&tokens);
        let is_libtessera = ridl_name.as_deref().is_some_and(is_libtessera_module);
        if module.header.profile == Profile::Kernel && is_libtessera {
            diagnostics.push(
                Diagnostic::error(V13, "kernel profile cannot import LibTessera")
                    .with_primary_span(item.span),
            );
        }
        if platform != Platform::Tessera && is_libtessera {
            diagnostics.push(
                Diagnostic::error(PLAT1, "non-Tessera modules cannot import LibTessera")
                    .with_primary_span(item.span),
            );
        }
        if is_platform_import(&tokens) {
            let library = string_at(&tokens, 2).unwrap_or_default();
            if !platform_library_matches(platform, library) {
                diagnostics.push(
                    Diagnostic::error(
                        PLAT1,
                        format!(
                            "platform library `{library}` is not valid for the {} platform",
                            platform.name()
                        ),
                    )
                    .with_primary_span(item.span),
                );
            }
        }
    }

    if !supports_foreign_interop(platform) {
        for item in &module.items {
            let Ok(tokens) = lex(module.item_text(*item)) else {
                continue;
            };
            if has_foreign_function_type(&tokens) {
                diagnostics.push(
                    Diagnostic::error(
                        PLAT1,
                        "foreign function types are only permitted in platform modules",
                    )
                    .with_primary_span(item.span),
                );
            }
        }
    }
    diagnostics
}

pub(super) fn verify_binary_platform(
    profile: Profile,
    platform: Platform,
    imports: &[BinaryImport],
    types: Option<&BinaryTypeTable>,
    code: Option<&BinaryCodeTable>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if platform == Platform::Portable && !matches!(profile, Profile::App | Profile::System) {
        diagnostics.push(Diagnostic::error(
            PLAT1,
            "portable modules must use the app or system profile",
        ));
    }
    for import in imports {
        let is_libtessera =
            import.kind == BinaryImportKind::Ridl && is_libtessera_module(&import.name);
        if profile == Profile::Kernel && is_libtessera {
            diagnostics.push(Diagnostic::error(
                V13,
                "kernel profile cannot import LibTessera",
            ));
        }
        if platform != Platform::Tessera && is_libtessera {
            diagnostics.push(Diagnostic::error(
                PLAT1,
                "non-Tessera modules cannot import LibTessera",
            ));
        }
        if import.kind == BinaryImportKind::Platform
            && !platform_library_matches(platform, &import.name)
        {
            diagnostics.push(Diagnostic::error(
                PLAT1,
                format!(
                    "platform library `{}` is not valid for the {} platform",
                    import.name,
                    platform.name()
                ),
            ));
        }
    }

    if !supports_foreign_interop(platform) {
        if types.is_some_and(|types| {
            types
                .entries
                .iter()
                .any(|ty| matches!(ty, BinaryType::Function(signature) if signature.foreign))
        }) {
            diagnostics.push(Diagnostic::error(
                PLAT1,
                "foreign function types are only permitted in platform modules",
            ));
        }
        if let Some(code) = code {
            for instruction in code
                .functions
                .iter()
                .flat_map(|function| &function.blocks)
                .flat_map(|block| block.instructions.iter().flatten())
                .filter(|instruction| {
                    instruction.opcode == 0x1a8 || (0x280..=0x282).contains(&instruction.opcode)
                })
            {
                diagnostics.push(Diagnostic::error(
                    PLAT1,
                    format!(
                        "foreign instruction opcode {:#x} at CODE offset {} is only permitted in platform modules",
                        instruction.opcode, instruction.offset
                    ),
                ));
            }
        }
    }

    if platform != Platform::Tessera
        && let Some(code) = code
    {
        for instruction in code
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| block.instructions.iter().flatten())
            .filter(|instruction| (0x270..=0x273).contains(&instruction.opcode))
        {
            diagnostics.push(Diagnostic::error(
                PLAT1,
                format!(
                    "device I/O or kernel opcode {:#x} at CODE offset {} requires the tessera platform",
                    instruction.opcode, instruction.offset
                ),
            ));
        }
    }
    diagnostics
}

pub(super) fn verify_text_body(platform: Platform, body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if !supports_foreign_interop(platform) {
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| {
                instruction.opcode == "call.foreign" || instruction.opcode.starts_with("foreign.")
            })
        {
            diagnostics.push(
                Diagnostic::error(
                    PLAT1,
                    "foreign calls and instructions are only permitted in platform modules",
                )
                .with_primary_span(instruction.span),
            );
        }
    }
    if platform != Platform::Tessera {
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| {
                matches!(
                    instruction.opcode.as_str(),
                    "mmio.load" | "mmio.store" | "fence.io" | "kernel.percpu.addr"
                )
            })
        {
            diagnostics.push(
                Diagnostic::error(
                    PLAT1,
                    "device I/O and kernel instructions require the tessera platform",
                )
                .with_primary_span(instruction.span),
            );
        }
    }
    diagnostics
}

fn supports_foreign_interop(platform: Platform) -> bool {
    platform != Platform::Portable
}

fn platform_library_matches(platform: Platform, library: &str) -> bool {
    match platform {
        Platform::Macos | Platform::Ios => {
            (library.starts_with("/usr/lib/") && library.ends_with(".dylib"))
                || (library.starts_with("/System/Library/Frameworks/")
                    && library.contains(".framework/"))
        }
        Platform::Windows => library.to_ascii_lowercase().ends_with(".dll"),
        Platform::Linux => library.contains(".so"),
        Platform::Android => library.starts_with("lib") && library.ends_with(".so"),
        Platform::Web => !library.is_empty(),
        Platform::Wasi => {
            let Some((namespace, rest)) = library.split_once(':') else {
                return false;
            };
            let Some((interface, version)) = rest.rsplit_once('@') else {
                return false;
            };
            !namespace.is_empty() && !interface.is_empty() && !version.is_empty()
        }
        Platform::Portable | Platform::Tessera => false,
    }
}

fn ridl_import_name(tokens: &[Token]) -> Option<String> {
    if atom_at(tokens, 0) != Some("import")
        || atom_at(tokens, 1) != Some("ridl")
        || !matches!(tokens.get(2)?.kind, TokenKind::Punctuation('@'))
    {
        return None;
    }
    let mut index = 3;
    let mut name = atom_at(tokens, index)?.to_owned();
    index += 1;
    while matches!(tokens.get(index)?.kind, TokenKind::Punctuation('.')) {
        name.push('.');
        index += 1;
        name.push_str(atom_at(tokens, index)?);
        index += 1;
    }
    (atom_at(tokens, index) == Some("abi")).then_some(name)
}

fn is_platform_import(tokens: &[Token]) -> bool {
    atom_at(tokens, 0) == Some("import") && atom_at(tokens, 1) == Some("platform")
}

fn has_foreign_function_type(tokens: &[Token]) -> bool {
    tokens.windows(5).any(|window| {
        atom_at(window, 0) == Some("abi")
            && matches!(window[1].kind, TokenKind::Punctuation('('))
            && atom_at(window, 2) == Some("c")
            && matches!(window[3].kind, TokenKind::Punctuation(')'))
            && atom_at(window, 4) == Some("foreign")
    })
}

fn string_at(tokens: &[Token], index: usize) -> Option<&str> {
    match &tokens.get(index)?.kind {
        TokenKind::String(value) => Some(value),
        _ => None,
    }
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    match &tokens.get(index)?.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

pub(super) fn is_libtessera_module(name: &str) -> bool {
    name == "LibTessera" || name.starts_with("LibTessera.")
}
