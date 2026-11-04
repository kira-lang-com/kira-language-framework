use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction};
use tir_target::Profile;
use tir_types::Type;
use tvisa_format::{ParameterItem, ResultItem, TextCallingConvention, TextSignature};

const VFY14: Code = Code::new(Dialect::Tvisa, Area::Verifier, 14);
const VFY40: Code = Code::new(Dialect::Tvisa, Area::Verifier, 40);

pub(super) fn verify_function(
    profile: Profile,
    source: &str,
    signature: &TextSignature,
    body: &FunctionBody,
) -> Vec<Diagnostic> {
    let gpu = matches!(
        signature.convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    );
    let mut diagnostics = Vec::new();
    let kernel_fp = has_attribute(source, "kernel_fp");
    let no_fp_regs = has_attribute(source, "no_fp_regs");

    if profile == Profile::Kernel && !kernel_fp && !no_fp_regs {
        diagnostics.push(
            Diagnostic::error(
                VFY14,
                "kernel functions without kernel_fp must carry the no_fp_regs attribute",
            )
            .with_primary_span(Span::new(0, source.len())),
        );
    } else if profile != Profile::Kernel && kernel_fp {
        diagnostics.push(
            Diagnostic::error(VFY14, "kernel_fp is only valid in the kernel profile")
                .with_primary_span(Span::new(0, source.len())),
        );
    }

    if profile == Profile::Kernel && !kernel_fp {
        let signature_uses_fp_or_vector = signature_uses_fp_or_vector(signature);
        let token_span = first_float_or_vector_token(source);
        if signature_uses_fp_or_vector || token_span.is_some() {
            diagnostics.push(
                Diagnostic::error(
                    VFY14,
                    "kernel functions without kernel_fp must not use floating-point or vector types or instructions",
                )
                .with_primary_span(token_span.unwrap_or(Span::new(0, source.len()))),
            );
        }
    }

    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode == "code.fromaddr"
                && !matches!(profile, Profile::System | Profile::Kernel)
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY40,
                        "code.fromaddr is only valid in system and kernel profiles",
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if !gpu && is_gpu_instruction(&instruction.opcode) {
                diagnostics.push(
                    Diagnostic::error(
                        VFY14,
                        format!(
                            "GPU instruction {} appears in a non-GPU function",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if profile == Profile::Kernel
                && runtime_entry_id(instruction).is_some_and(is_allocation_runtime_entry)
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY14,
                        "kernel functions must not call runtime allocation entries",
                    )
                    .with_primary_span(instruction.span),
                );
            }
        }
    }

    diagnostics
}

fn signature_uses_fp_or_vector(signature: &TextSignature) -> bool {
    signature.parameters.iter().any(parameter_uses_fp_or_vector)
        || signature.results.iter().any(result_uses_fp_or_vector)
}

fn parameter_uses_fp_or_vector(parameter: &ParameterItem) -> bool {
    match parameter {
        ParameterItem::Leaf(ty) => type_uses_fp_or_vector(ty),
        ParameterItem::Group(group) => group
            .members
            .iter()
            .any(|member| type_uses_fp_or_vector(&member.ty)),
        ParameterItem::CAggregate(aggregate) => aggregate
            .members
            .iter()
            .any(|member| type_uses_fp_or_vector(&member.ty)),
        ParameterItem::ByRef { .. } | ParameterItem::Sret { .. } | ParameterItem::Context => false,
    }
}

fn result_uses_fp_or_vector(result: &ResultItem) -> bool {
    match result {
        ResultItem::Leaf(ty) => type_uses_fp_or_vector(ty),
        ResultItem::Group(group) => group
            .members
            .iter()
            .any(|member| type_uses_fp_or_vector(&member.ty)),
    }
}

fn type_uses_fp_or_vector(ty: &Type) -> bool {
    match ty {
        Type::Float(_) | Type::FixedVector { .. } | Type::ScalableVector(_) => true,
        Type::Tuple(elements) => elements.iter().any(type_uses_fp_or_vector),
        Type::Array { element, .. }
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Tensor { element, .. } => type_uses_fp_or_vector(element),
        Type::Nominal { arguments, .. } => arguments.iter().any(type_uses_fp_or_vector),
        Type::Address { pointee, .. } => type_uses_fp_or_vector(pointee),
        Type::Function(function) => function_uses_fp_or_vector(function),
        Type::VisibleFunctionTable(function) => function_uses_fp_or_vector(function),
        Type::Closure(closure) => {
            closure
                .signature
                .parameters
                .iter()
                .any(|parameter| type_uses_fp_or_vector(&parameter.ty))
                || type_uses_fp_or_vector(&closure.signature.result)
                || closure
                    .signature
                    .failure
                    .as_deref()
                    .is_some_and(type_uses_fp_or_vector)
        }
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            type_uses_fp_or_vector(yield_type)
                || type_uses_fp_or_vector(resume_type)
                || type_uses_fp_or_vector(result_type)
        }
        Type::Texture { component, .. } => type_uses_fp_or_vector(component),
        Type::CooperativeMatrix { element, .. } => type_uses_fp_or_vector(element),
        Type::Entry {
            root, immediate, ..
        } => type_uses_fp_or_vector(root) || type_uses_fp_or_vector(immediate),
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

fn function_uses_fp_or_vector(function: &tir_types::FunctionType) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| type_uses_fp_or_vector(&parameter.ty))
        || type_uses_fp_or_vector(&function.result)
        || function
            .failure
            .as_deref()
            .is_some_and(type_uses_fp_or_vector)
}

fn first_float_or_vector_token(source: &str) -> Option<Span> {
    let tokens = lex(source).ok()?;
    tokens.iter().enumerate().find_map(|(index, token)| {
        let TokenKind::Atom(atom) = &token.kind else {
            return None;
        };
        if matches!(
            tokens.get(index.wrapping_sub(1)).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%' | '@' | '$' | '.'))
        ) {
            return None;
        }
        let is_float = matches!(
            atom.as_str(),
            "f16"
                | "bf16"
                | "f32"
                | "f64"
                | "f128"
                | "f8e4m3"
                | "f8e5m2"
                | "f6e2m3"
                | "f6e3m2"
                | "f4e2m1"
                | "e8m0"
        );
        let is_vector = matches!(atom.as_str(), "vec" | "svec");
        (is_float || is_vector).then_some(token.span)
    })
}

fn has_attribute(source: &str, attribute: &str) -> bool {
    let Ok(tokens) = lex(source) else {
        return false;
    };
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    tokens[..header_end].windows(2).any(|pair| {
        matches!(&pair[0].kind, TokenKind::Atom(name) if name == attribute)
            && matches!(pair[1].kind, TokenKind::Punctuation('(' | ',' | ')'))
    })
}

fn is_gpu_instruction(opcode: &str) -> bool {
    if opcode == "gpu.entry.ref" {
        return false;
    }
    matches!(opcode, "const.sampler")
        || [
            "gpu.",
            "tex.",
            "quad.",
            "subgroup.",
            "object.",
            "mesh.",
            "rq.",
            "rt.",
            "ray.",
            "coop.",
            "tensor.",
            "tile.",
            "node.",
            "frag.",
        ]
        .iter()
        .any(|prefix| opcode.starts_with(prefix))
}

pub(super) fn runtime_entry_id(instruction: &TextInstruction) -> Option<u16> {
    if instruction.opcode != "call.runtime" {
        return None;
    }
    let tokens = &instruction.tokens;
    let mut index = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        while index < tokens.len() {
            if matches!(tokens[index].kind, TokenKind::Punctuation('=')) {
                index += 1;
                break;
            }
            index += 1;
        }
    }
    index += 1;
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) && index + 1 < tokens.len()
    {
        index += 2;
    }
    let TokenKind::Atom(value) = &tokens.get(index)?.kind else {
        return None;
    };
    tir_format::numeric_literals::parse_unsigned_integer(value)
        .and_then(|value| u16::try_from(value).ok())
}

const fn is_allocation_runtime_entry(id: u16) -> bool {
    matches!(id, 0x0001..=0x0004 | 0x0020)
}
