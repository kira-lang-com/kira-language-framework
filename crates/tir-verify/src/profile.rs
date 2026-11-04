use std::collections::HashSet;
use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    BinaryProducer, FunctionBody, ProducerRecord, TextFunctionSignature, TextItemKind, parse_type,
};
use tir_target::{Platform, Profile};

use tir_types::{FunctionType, Type};

const V13: Code = Code::new(Dialect::Tir, Area::Verifier, 13);
const PROF10: Code = Code::new(Dialect::Tir, Area::Profile, 10);
const PROF12: Code = Code::new(Dialect::Tir, Area::Profile, 12);
const PROF13: Code = Code::new(Dialect::Tir, Area::Profile, 13);
const PLAT4: Code = Code::new(Dialect::Tir, Area::Platform, 4);

const DRIVER_LANGUAGES: &[&str] = &["kira", "rust", "zig", "c", "cpp", "karc-cpp", "karc-arc"];

pub(super) fn verify_module_profile(
    profile: Profile,
    platform: Platform,
    languages: Option<&[String]>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if matches!(profile, Profile::Driver | Profile::Kernel) && platform != Platform::Tessera {
        diagnostics.push(Diagnostic::error(
            PLAT4,
            format!(
                "the {} profile requires the tessera platform",
                profile.name()
            ),
        ));
    }
    let Some(languages) = languages else {
        let code = match profile {
            Profile::Driver => Some(PROF10),
            Profile::Kernel => Some(PROF12),
            Profile::App | Profile::System => None,
        };
        if let Some(code) = code {
            diagnostics.push(Diagnostic::error(
                code,
                format!(
                    "the {} profile requires a source-language producer record",
                    profile.name()
                ),
            ));
        }
        return diagnostics;
    };
    if languages.is_empty() && matches!(profile, Profile::Driver | Profile::Kernel) {
        let code = if profile == Profile::Driver {
            PROF10
        } else {
            PROF12
        };
        diagnostics.push(Diagnostic::error(
            code,
            format!(
                "the {} profile requires at least one source language",
                profile.name()
            ),
        ));
        return diagnostics;
    }
    let allowed = match profile {
        Profile::Driver => Some(DRIVER_LANGUAGES),
        Profile::Kernel => Some(&["kira", "rust"][..]),
        Profile::App | Profile::System => None,
    };
    if let Some(allowed) = allowed
        && languages
            .iter()
            .any(|language| !allowed.contains(&language.as_str()))
    {
        let code = if profile == Profile::Driver {
            PROF10
        } else {
            PROF12
        };
        diagnostics.push(Diagnostic::error(
            code,
            format!(
                "the {} profile does not accept source languages [{}]",
                profile.name(),
                languages.join(", ")
            ),
        ));
    }
    diagnostics
}

pub(super) fn verify_text_module_profile(
    profile: Profile,
    platform: Option<Platform>,
    producer: Option<&ProducerRecord>,
    source_hash: Option<&str>,
) -> Vec<Diagnostic> {
    let mut diagnostics = verify_module_profile(
        profile,
        platform.unwrap_or(Platform::Portable),
        producer.map(|producer| producer.languages.as_slice()),
    );
    verify_source_hash(profile, source_hash, &mut diagnostics);
    diagnostics
}

pub(super) fn verify_binary_module_profile(
    profile: Profile,
    platform: Platform,
    producer: Option<&BinaryProducer>,
    source_hash_present: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = verify_module_profile(
        profile,
        platform,
        producer.map(|producer| producer.languages.as_slice()),
    );
    if profile == Profile::Driver && !source_hash_present {
        diagnostics.push(Diagnostic::error(
            PROF13,
            "driver module is missing its source-reproducibility hash",
        ));
    }
    diagnostics
}

fn verify_source_hash(
    profile: Profile,
    source_hash: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match (profile, source_hash) {
        (Profile::Driver, None) => diagnostics.push(Diagnostic::error(
            PROF13,
            "driver module is missing its source-reproducibility hash",
        )),
        (Profile::Driver, Some(hash))
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            diagnostics.push(Diagnostic::error(
                PROF13,
                "driver source-reproducibility hash must be 64 lowercase hexadecimal digits",
            ));
        }
        (Profile::App | Profile::System | Profile::Kernel, Some(_)) => {
            diagnostics.push(Diagnostic::error(
                PROF13,
                "source-reproducibility hashes are only permitted in driver modules",
            ));
        }
        _ => {}
    }
}

pub(super) fn verify_header(profile: Profile, kind: TextItemKind, source: &str) -> Vec<Diagnostic> {
    let Ok(tokens) = lex(source) else {
        return Vec::new();
    };
    let header = &tokens[..header_end(&tokens)];
    let mut diagnostics = Vec::new();

    if profile == Profile::Kernel && kind == TextItemKind::Entry {
        diagnostics.push(
            Diagnostic::error(V13, "GPU entries are forbidden in the kernel profile")
                .with_primary_span(header.first().map_or(Span::new(0, 0), |token| token.span)),
        );
    }
    if profile == Profile::Kernel && kind == TextItemKind::Function && !has_atom(header, "nounwind")
    {
        diagnostics.push(
            Diagnostic::error(V13, "every kernel function must be marked nounwind")
                .with_primary_span(header.first().map_or(Span::new(0, 0), |token| token.span)),
        );
    }
    if matches!(profile, Profile::Driver | Profile::Kernel)
        && has_engine(header, &["runtime", "any"])
    {
        diagnostics.push(
            Diagnostic::error(V13, "runtime and any engines are forbidden by this profile")
                .with_primary_span(
                    find_atom(header, "engine").unwrap_or_else(|| {
                        header.first().map_or(Span::new(0, 0), |token| token.span)
                    }),
                ),
        );
    }
    if profile != Profile::Kernel && has_qualified_atom(header, "kernel", "fp") {
        diagnostics.push(
            Diagnostic::error(V13, "kernel.fp is only permitted in the kernel profile")
                .with_primary_span(
                    find_atom(header, "kernel").unwrap_or_else(|| {
                        header.first().map_or(Span::new(0, 0), |token| token.span)
                    }),
                ),
        );
    }
    diagnostics
}

pub(super) fn verify_body(
    profile: Profile,
    platform: Platform,
    source: &str,
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
) -> Vec<Diagnostic> {
    let Ok(tokens) = lex(source) else {
        return Vec::new();
    };
    let header = &tokens[..header_end(&tokens)];
    let kernel_fp = profile == Profile::Kernel && has_qualified_atom(header, "kernel", "fp");
    let mut diagnostics = Vec::new();
    let static_integer_values = if profile == Profile::Kernel {
        constant_integer_results(source, body)
    } else {
        HashSet::new()
    };

    if profile == Profile::Kernel && !kernel_fp {
        if let Some(signature) = signature {
            for parameter in &signature.parameters {
                if contains_float_or_vector(&parameter.ty) {
                    diagnostics.push(
                        Diagnostic::error(V13, "floating-point and vector types require kernel.fp")
                            .with_primary_span(parameter.span),
                    );
                }
            }
            if contains_float_or_vector(&signature.result) {
                diagnostics.push(
                    Diagnostic::error(V13, "floating-point and vector types require kernel.fp")
                        .with_primary_span(
                            header.first().map_or(Span::new(0, 0), |token| token.span),
                        ),
                );
            }
        }
        for parameter in body.blocks.iter().flat_map(|block| &block.parameters) {
            if contains_float_or_vector(&parameter.ty) {
                diagnostics.push(
                    Diagnostic::error(V13, "floating-point and vector types require kernel.fp")
                        .with_primary_span(parameter.span),
                );
            }
        }
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let opcode = instruction.opcode.as_str();
        let forbidden = match profile {
            Profile::App | Profile::System if is_device_io(opcode) => {
                Some("device I/O instructions are forbidden in the app and system profiles")
            }
            Profile::Kernel if is_unwinding(opcode) => {
                Some("unwinding instructions are forbidden in the kernel profile")
            }
            _ => None,
        };
        if let Some(message) = forbidden {
            diagnostics.push(Diagnostic::error(V13, message).with_primary_span(instruction.span));
        }
        if opcode.starts_with("kernel.") && profile != Profile::Kernel {
            diagnostics.push(
                Diagnostic::error(
                    V13,
                    "kernel instructions are only permitted in the kernel profile",
                )
                .with_primary_span(instruction.span),
            );
        }
        if profile == Profile::Kernel
            && !kernel_fp
            && instruction_uses_float_or_vector(source, instruction)
        {
            diagnostics.push(
                Diagnostic::error(
                    V13,
                    "floating-point and vector instructions require kernel.fp",
                )
                .with_primary_span(instruction.span),
            );
        }
        if profile == Profile::Kernel
            && opcode == "stack.alloc"
            && has_dynamic_count(instruction, &static_integer_values)
        {
            diagnostics.push(
                Diagnostic::error(V13, "kernel stack.alloc sizes must be static")
                    .with_primary_span(instruction.span),
            );
        }
    }
    diagnostics.extend(crate::platform::verify_text_body(platform, body));
    diagnostics
}

pub(super) fn constant_integer_results(source: &str, body: &FunctionBody) -> HashSet<String> {
    body.blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| instruction.opcode == "const.int" && instruction.results.len() == 1)
        .filter(|instruction| {
            let start = crate::ssa::text_instruction_operands_start(&instruction.tokens);
            let Some(comma) = instruction.tokens[start..]
                .iter()
                .position(|token| matches!(token.kind, TokenKind::Punctuation(',')))
                .map(|index| start + index)
            else {
                return false;
            };
            let Some(value) = instruction.tokens.get(comma + 1) else {
                return false;
            };
            let Some(text) = source.get(value.span.start..instruction.span.end) else {
                return false;
            };
            parse_integer_literal(text.trim())
                .is_some_and(|value| (0..(1_i128 << 62)).contains(&value))
        })
        .map(|instruction| instruction.results[0].clone())
        .collect()
}

pub(super) fn has_dynamic_count(
    instruction: &tir_format::TextInstruction,
    static_integer_values: &HashSet<String>,
) -> bool {
    instruction.tokens.windows(4).any(|window| {
        matches!(window[0].kind, TokenKind::Punctuation('['))
            && matches!(&window[1].kind, TokenKind::Atom(atom) if atom == "count")
            && matches!(window[2].kind, TokenKind::Punctuation('%'))
            && matches!(&window[3].kind, TokenKind::Atom(name) if !static_integer_values.contains(name))
    })
}

fn parse_integer_literal(text: &str) -> Option<i128> {
    tir_format::numeric_literals::parse_signed_integer(text)
}

fn header_end(tokens: &[Token]) -> usize {
    tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len())
}

fn has_atom(tokens: &[Token], name: &str) -> bool {
    tokens
        .iter()
        .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == name))
}

fn find_atom(tokens: &[Token], name: &str) -> Option<Span> {
    tokens.iter().find_map(|token| {
        matches!(&token.kind, TokenKind::Atom(atom) if atom == name).then_some(token.span)
    })
}

fn has_engine(tokens: &[Token], modes: &[&str]) -> bool {
    tokens.windows(4).any(|window| {
        matches!(&window[0].kind, TokenKind::Atom(atom) if atom == "engine")
            && matches!(window[1].kind, TokenKind::Punctuation('('))
            && matches!(&window[2].kind, TokenKind::Atom(mode) if modes.contains(&mode.as_str()))
            && matches!(window[3].kind, TokenKind::Punctuation(')'))
    })
}

fn has_qualified_atom(tokens: &[Token], prefix: &str, name: &str) -> bool {
    tokens.windows(3).any(|window| {
        matches!(&window[0].kind, TokenKind::Atom(atom) if atom == prefix)
            && matches!(window[1].kind, TokenKind::Punctuation('.'))
            && matches!(&window[2].kind, TokenKind::Atom(atom) if atom == name)
    })
}

fn is_device_io(opcode: &str) -> bool {
    matches!(opcode, "mmio.load" | "mmio.store" | "fence.io")
}

fn is_unwinding(opcode: &str) -> bool {
    matches!(opcode, "invoke" | "landingpad" | "resume" | "throw")
}

fn instruction_uses_float_or_vector(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> bool {
    let opcode = instruction.opcode.as_str();
    if is_float_instruction(opcode) || opcode.starts_with("vec.") || opcode.starts_with("svec.") {
        return true;
    }
    instruction_has_float_or_vector_type(source, instruction)
}

fn is_float_instruction(opcode: &str) -> bool {
    matches!(
        opcode,
        "const.float"
            | "fadd"
            | "fsub"
            | "fmul"
            | "fdiv"
            | "frem"
            | "fneg"
            | "fabs"
            | "fsqrt"
            | "fma"
            | "fmuladd"
            | "fmin"
            | "fmax"
            | "fminnum"
            | "fmaxnum"
            | "fcopysign"
            | "ffloor"
            | "fceil"
            | "ftrunc"
            | "fround"
            | "froundeven"
            | "fcmp"
            | "fclass"
            | "frexp"
            | "ldexp"
            | "fp.flags.read"
            | "fp.flags.clear"
            | "fptrunc"
            | "fpext"
            | "sitofp"
            | "uitofp"
            | "fptosi"
            | "fptoui"
            | "fcvt"
    ) || opcode.starts_with("math.")
}

fn instruction_has_float_or_vector_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> bool {
    let start = crate::ssa::text_instruction_operands_start(&instruction.tokens);
    let mut starts = vec![start];
    for (index, token) in instruction.tokens.iter().enumerate() {
        if matches!(&token.kind, TokenKind::Atom(atom) if matches!(atom.as_str(), "to" | "as"))
            || matches!(token.kind, TokenKind::Punctuation(':'))
        {
            starts.push(index + 1);
        }
    }
    for start in starts {
        let Some(first) = instruction.tokens.get(start) else {
            continue;
        };
        for end in start + 1..=instruction.tokens.len() {
            let Some(last) = instruction.tokens.get(end - 1) else {
                break;
            };
            let Some(text) = source.get(first.span.start..last.span.end) else {
                break;
            };
            if parse_type(text).is_ok_and(|ty| contains_float_or_vector(&ty)) {
                return true;
            }
        }
    }
    false
}

fn contains_float_or_vector(ty: &Type) -> bool {
    match ty {
        Type::Float(_) | Type::FixedVector { .. } | Type::ScalableVector(_) => true,
        Type::Tuple(elements) => elements.iter().any(contains_float_or_vector),
        Type::Array { element, .. }
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Address {
            pointee: element, ..
        }
        | Type::Tensor { element, .. }
        | Type::CooperativeMatrix { element, .. } => contains_float_or_vector(element),
        Type::Nominal { arguments, .. } => arguments.iter().any(contains_float_or_vector),
        Type::Function(signature) => function_type_contains_float_or_vector(signature),
        Type::VisibleFunctionTable(signature) => function_type_contains_float_or_vector(signature),
        Type::Closure(closure) => function_type_contains_float_or_vector(&closure.signature),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            contains_float_or_vector(yield_type)
                || contains_float_or_vector(resume_type)
                || contains_float_or_vector(result_type)
        }
        Type::Texture { component, .. } => contains_float_or_vector(component),
        Type::Entry {
            root, immediate, ..
        } => contains_float_or_vector(root) || contains_float_or_vector(immediate),
        Type::Integer(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::Generic(_)
        | Type::Frame(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => false,
    }
}

fn function_type_contains_float_or_vector(signature: &FunctionType) -> bool {
    signature
        .parameters
        .iter()
        .any(|parameter| contains_float_or_vector(&parameter.ty))
        || contains_float_or_vector(&signature.result)
        || signature
            .failure
            .as_deref()
            .is_some_and(contains_float_or_vector)
}
