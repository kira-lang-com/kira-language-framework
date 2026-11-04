use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::lex;
use tir_target::{Architecture, known_features};
use tvisa_format::{
    TextCallingConvention, TextImage, TextItem, TextItemKind, TextSignature,
    TextVariantArchitecture as VariantArchitecture, TextVariantKey as VariantKey,
    TextVariantMember as VariantMember, TvisaImage,
};

use crate::assembly_constraints::{AssemblyArchitecture, verify_text_instruction};
use crate::variant_text::{
    atom, intrinsic_architecture, is_atom, is_punctuation, parse_function_variant, read_symbol,
    significant,
};

const VFY12: Code = Code::new(Dialect::Tvisa, Area::Verifier, 12);
const ARCH1: Code = Code::new(Dialect::Tvisa, Area::Architecture, 1);
const ARCH2: Code = Code::new(Dialect::Tvisa, Area::Architecture, 2);
const ARCH3: Code = Code::new(Dialect::Tvisa, Area::Architecture, 3);
const HAS_VARIANTS: u32 = 1 << 2;
const HAS_DISPATCH: u32 = 1 << 3;

#[derive(Clone, Debug)]
struct FunctionRecord {
    signature: Option<String>,
    variant: Option<VariantKey>,
    body_has_assembly: bool,
    span: Span,
}

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut functions = HashMap::new();
    let mut variant_membership: HashMap<String, Vec<Span>> = HashMap::new();

    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        if let Some((name, record)) = parse_function(image, *item, &mut diagnostics) {
            functions.insert(name, record);
        }
    }

    for set in &image.variant_sets {
        verify_set(
            image,
            set.span,
            &set.members,
            &functions,
            &mut variant_membership,
            &mut diagnostics,
        );
    }

    for (name, record) in &functions {
        if record.variant.is_some() && !variant_membership.contains_key(name) {
            push(
                &mut diagnostics,
                0,
                record.span,
                format!("variant function @{name} is not a member of a variant or dispatch set"),
            );
        } else if record.variant.is_some()
            && variant_membership
                .get(name)
                .is_some_and(|sets| sets.len() > 1)
        {
            push(
                &mut diagnostics,
                0,
                record.span,
                format!("variant function @{name} belongs to more than one set"),
            );
        }
    }

    diagnostics
}

pub(super) fn verify_binary(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    if let Err(error) = image.validate_signature_indices(memory_limit) {
        return vec![error.diagnostic];
    }
    let variants = match image.variant_index(memory_limit) {
        Ok(variants) => variants,
        Err(error) => return vec![error.diagnostic],
    };
    let imports = match image.import_counts(memory_limit) {
        Ok(imports) => imports,
        Err(error) => return vec![error.diagnostic],
    };
    let functions = match image.function_records(memory_limit) {
        Ok(functions) => functions,
        Err(error) => return vec![error.diagnostic],
    };
    let bodies = match image.function_bodies(memory_limit) {
        Ok(bodies) => bodies,
        Err(error) => return vec![error.diagnostic],
    };
    let assembly_functions: HashSet<_> = bodies
        .iter()
        .filter(|body| {
            body.blocks.iter().any(|block| {
                block
                    .instructions
                    .iter()
                    .any(|instruction| matches!(instruction.opcode, 0x220 | 0x221))
            })
        })
        .map(|body| body.header.function_index)
        .collect();
    let mut diagnostics = Vec::new();
    let has_sets = !variants.sets.is_empty();
    let has_dispatch = variants.sets.iter().any(|set| set.kind == 1);
    if (image.header.flags & HAS_VARIANTS != 0) != has_sets {
        binary_error(
            &mut diagnostics,
            "header variant-set flag disagrees with VARIANTS",
        );
    }
    if (image.header.flags & HAS_DISPATCH != 0) != has_dispatch {
        binary_error(
            &mut diagnostics,
            "header dispatch-set flag disagrees with VARIANTS",
        );
    }

    let mut membership = HashMap::<u64, usize>::new();
    for set in &variants.sets {
        let mut keys = HashSet::new();
        let mut members = HashSet::new();
        let mut signature = None;
        let mut has_portable = false;
        let mut has_three_bases = [false; 3];
        let mut set_has_assembly = false;
        for member in &set.members {
            let mut features = member.features.clone();
            features.sort_unstable();
            if !keys.insert((member.architecture, features.clone())) {
                binary_error(
                    &mut diagnostics,
                    format!(
                        "variant set @{} repeats an architecture and feature key",
                        set.name
                    ),
                );
            }
            if !members.insert(member.function_index) {
                binary_error(
                    &mut diagnostics,
                    format!("variant set @{} repeats a function member", set.name),
                );
            }
            *membership.entry(member.function_index).or_default() += 1;
            set_has_assembly |= assembly_functions.contains(&member.function_index);
            validate_binary_features(
                member.architecture,
                &member.features,
                &set.name,
                &mut diagnostics,
            );
            has_portable |= member.architecture == 0;
            if (1..=3).contains(&member.architecture) && member.features.is_empty() {
                has_three_bases[usize::from(member.architecture - 1)] = true;
            }

            let Some(function_index) = member.function_index.checked_sub(imports.functions) else {
                binary_error(
                    &mut diagnostics,
                    format!("variant set @{} references an imported function", set.name),
                );
                continue;
            };
            let Some(function) = usize::try_from(function_index)
                .ok()
                .and_then(|index| functions.get(index))
            else {
                binary_error(
                    &mut diagnostics,
                    format!("variant set @{} references an invalid function", set.name),
                );
                continue;
            };
            if function.target != 0 {
                binary_error(
                    &mut diagnostics,
                    format!(
                        "variant set member @{} is not a CPU function",
                        function.name
                    ),
                );
            }
            if let Some(expected) = signature {
                if expected != function.signature_index {
                    binary_error(
                        &mut diagnostics,
                        format!("variant set @{} has incompatible signatures", set.name),
                    );
                }
            } else {
                signature = Some(function.signature_index);
            }
            match &function.variant {
                Some(variant)
                    if variant.architecture == member.architecture
                        && same_features(&variant.features, &member.features) => {}
                _ => binary_error(
                    &mut diagnostics,
                    format!(
                        "variant set @{} member @{} disagrees with its function variant",
                        set.name, function.name
                    ),
                ),
            }
        }

        if (set_has_assembly && !has_three_bases.into_iter().all(|base| base))
            || (!set_has_assembly && !has_portable && !has_three_bases.into_iter().all(|base| base))
        {
            binary_error(
                &mut diagnostics,
                if set_has_assembly {
                    format!(
                        "variant set @{} containing assembly requires feature-free aarch64, x64, and rv64 members",
                        set.name
                    )
                } else {
                    format!(
                        "variant set @{} without assembly requires a portable member or feature-free aarch64, x64, and rv64 members",
                        set.name
                    )
                },
            );
        }
    }

    for (index, function) in functions.iter().enumerate() {
        let function_index = imports.functions.saturating_add(index as u64);
        let count = membership.get(&function_index).copied().unwrap_or(0);
        match (&function.variant, count) {
            (Some(_), 0) => binary_error(
                &mut diagnostics,
                format!(
                    "variant function @{} is not in a variant set",
                    function.name
                ),
            ),
            (Some(_), 1) | (None, 0) => {}
            (Some(_), 2..) => binary_error(
                &mut diagnostics,
                format!(
                    "variant function @{} belongs to multiple sets",
                    function.name
                ),
            ),
            (None, 1..) => binary_error(
                &mut diagnostics,
                format!(
                    "ordinary function @{} appears in a variant set",
                    function.name
                ),
            ),
        }
        if let Some(variant) = &function.variant {
            validate_binary_features(
                variant.architecture,
                &variant.features,
                &function.name,
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

fn validate_binary_features(
    architecture_code: u8,
    features: &[String],
    owner: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if architecture_code == 0 {
        if !features.is_empty() {
            binary_error(
                diagnostics,
                format!("portable variant @{} cannot require features", owner),
            );
        }
        return;
    }
    let architecture = match architecture_code {
        1 => Architecture::Aarch64,
        2 => Architecture::X64,
        3 => Architecture::Rv64,
        4 => Architecture::Wasm,
        _ => return,
    };
    let known = known_features(architecture);
    for feature in features {
        if !known.contains(&feature.as_str()) {
            binary_error(
                diagnostics,
                format!(
                    "variant @{} uses unknown {} feature {feature}",
                    owner,
                    architecture.name()
                ),
            );
        }
    }
}

fn same_features(left: &[String], right: &[String]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

fn binary_error(diagnostics: &mut Vec<Diagnostic>, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(VFY12, message));
}

fn parse_function(
    image: &TextImage,
    item: TextItem,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(String, FunctionRecord)> {
    let source = image.item_text(item);
    let tokens = match lex(source) {
        Ok(tokens) => significant(tokens),
        Err(error) => {
            push(diagnostics, item.span.start, error.span, error.message);
            return None;
        }
    };
    let header_end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let Some(symbol_index) = header.iter().position(|token| is_punctuation(token, '@')) else {
        return None;
    };
    let Some((name, _, symbol_span)) = read_symbol(header, symbol_index) else {
        return None;
    };
    let signature = header.windows(3).find_map(|window| {
        (is_atom(&window[0], "sig") && is_punctuation(&window[1], '$'))
            .then(|| atom(&window[2]).map(str::to_owned))
            .flatten()
    });
    let variant = match parse_function_variant(header) {
        Ok(variant) => variant,
        Err((span, message)) => {
            push(diagnostics, item.span.start, span, message);
            None
        }
    };
    let body_has_assembly = image
        .function_body_for_item(item)
        .and_then(|parsed| parsed.body.as_ref())
        .is_some_and(|body| {
            let mut has_assembly = false;
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                match instruction.opcode.as_str() {
                    "asm" | "asm.goto" => {
                        has_assembly = true;
                        match &variant {
                            Some(key) if key.architecture != VariantArchitecture::Portable => {
                                match AssemblyArchitecture::from_variant(key.architecture) {
                                    Some(architecture) => {
                                        if let Err(message) =
                                            verify_text_instruction(instruction, architecture)
                                        {
                                            push_with_code(
                                                diagnostics,
                                                ARCH3,
                                                item.span.start,
                                                instruction.span,
                                                message,
                                            );
                                        }
                                    }
                                    None => push_with_code(
                                        diagnostics,
                                        ARCH1,
                                        item.span.start,
                                        instruction.span,
                                        "asm and asm.goto require an aarch64, x64 or rv64 variant body",
                                    ),
                                }
                            }
                            _ => push_with_code(
                                diagnostics,
                                ARCH1,
                                item.span.start,
                                instruction.span,
                                "asm and asm.goto require a non-portable variant body",
                            ),
                        }
                    }
                    "target.intrinsic" => {
                        if let Some(intrinsic_arch) = intrinsic_architecture(&instruction.tokens) {
                            if variant.as_ref().map(|key| key.architecture) != Some(intrinsic_arch)
                            {
                                push_with_code(
                                    diagnostics,
                                    ARCH2,
                                    item.span.start,
                                    instruction.span,
                                    format!(
                                        "{} target intrinsic must be in a {} variant body",
                                        intrinsic_arch.name(),
                                        intrinsic_arch.name()
                                    ),
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            has_assembly
        });
    Some((
        name,
        FunctionRecord {
            signature,
            variant,
            body_has_assembly,
            span: shift(symbol_span, item.span.start),
        },
    ))
}

fn verify_set(
    image: &TextImage,
    set_span: Span,
    members: &[VariantMember],
    functions: &HashMap<String, FunctionRecord>,
    membership: &mut HashMap<String, Vec<Span>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen_keys = HashSet::new();
    let mut seen_functions = HashSet::new();
    let mut reference_signature: Option<TextSignature> = None;
    let mut has_assembly = false;

    for member in members {
        if !seen_keys.insert(member.key.clone()) {
            push(
                diagnostics,
                0,
                member.span,
                "duplicate architecture and feature variant in set",
            );
        }
        if !seen_functions.insert(member.function.clone()) {
            push(
                diagnostics,
                0,
                member.span,
                format!(
                    "function @{} appears more than once in this set",
                    member.function
                ),
            );
        }
        membership
            .entry(member.function.clone())
            .or_default()
            .push(member.span);

        validate_features(&member.key, member.span, 0, diagnostics);

        let Some(function) = functions.get(&member.function) else {
            push(
                diagnostics,
                0,
                member.span,
                format!(
                    "variant member @{} must name a defined TVISA function",
                    member.function
                ),
            );
            continue;
        };
        has_assembly |= function.body_has_assembly;
        if !same_variant(&member.key, function.variant.as_ref()) {
            push(
                diagnostics,
                0,
                member.span,
                format!(
                    "variant member @{} must have a matching variant attribute",
                    member.function
                ),
            );
        }
        let Some(signature_name) = function.signature.as_deref() else {
            push(
                diagnostics,
                0,
                member.span,
                format!("variant member @{} has no signature", member.function),
            );
            continue;
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|signature| signature.name == signature_name)
        else {
            continue;
        };
        if matches!(
            signature.convention,
            TextCallingConvention::Gpu | TextCallingConvention::Entry
        ) {
            push(
                diagnostics,
                0,
                member.span,
                format!(
                    "variant member @{} must use a native CPU signature",
                    member.function
                ),
            );
        }
        if let Some(reference) = &reference_signature {
            if !same_signature(reference, signature) {
                push(
                    diagnostics,
                    0,
                    member.span,
                    format!(
                        "variant member @{} must have the same signature and ABI as its set",
                        member.function
                    ),
                );
            }
        } else {
            reference_signature = Some(signature.clone());
        }
    }

    let has_portable = members
        .iter()
        .any(|member| member.key.architecture == VariantArchitecture::Portable);
    let has_three_bases = [
        VariantArchitecture::Aarch64,
        VariantArchitecture::X64,
        VariantArchitecture::Rv64,
    ]
    .into_iter()
    .all(|architecture| {
        members
            .iter()
            .any(|member| member.key.architecture == architecture && member.key.features.is_empty())
    });

    if has_assembly {
        if !has_three_bases {
            push(
                diagnostics,
                0,
                set_span,
                "a variant set containing assembly requires feature-free aarch64, x64, and rv64 members",
            );
        }
    } else if !has_portable && !has_three_bases {
        push(
            diagnostics,
            0,
            set_span,
            "a variant set without assembly requires a portable member or feature-free aarch64, x64, and rv64 members",
        );
    }
}

fn validate_features(
    key: &VariantKey,
    span: Span,
    offset: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if key.architecture == VariantArchitecture::Portable && !key.features.is_empty() {
        push(
            diagnostics,
            offset,
            span,
            "portable variants cannot name architecture features",
        );
        return;
    }
    let Some(architecture) = key.architecture.target() else {
        return;
    };
    let known = known_features(architecture);
    let mut seen = HashSet::new();
    for feature in &key.features {
        if !seen.insert(feature) {
            push(
                diagnostics,
                offset,
                span,
                format!("duplicate feature {feature} in variant"),
            );
        } else if !known.contains(&feature.as_str()) {
            push(
                diagnostics,
                offset,
                span,
                format!(
                    "unknown {} feature {feature} in variant",
                    key.architecture.name()
                ),
            );
        }
    }
}

fn same_signature(left: &TextSignature, right: &TextSignature) -> bool {
    left.convention == right.convention
        && left.fails == right.fails
        && left.variadic == right.variadic
        && left.noreturn == right.noreturn
        && left.parameters == right.parameters
        && left.results == right.results
}

fn same_variant(expected: &VariantKey, actual: Option<&VariantKey>) -> bool {
    let Some(actual) = actual else {
        return expected.architecture == VariantArchitecture::Portable
            && expected.features.is_empty();
    };
    expected.architecture == actual.architecture
        && same_feature_set(&expected.features, &actual.features)
}

fn same_feature_set(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().all(|feature| right.contains(feature))
}

fn push(diagnostics: &mut Vec<Diagnostic>, offset: usize, span: Span, message: impl Into<String>) {
    push_with_code(diagnostics, VFY12, offset, span, message);
}

fn push_with_code(
    diagnostics: &mut Vec<Diagnostic>,
    code: Code,
    offset: usize,
    span: Span,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(code, message).with_primary_span(shift(span, offset)));
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
