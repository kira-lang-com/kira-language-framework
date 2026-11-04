use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::{AddressSpace, Type};

use crate::gpu_profile::policy::TypePolicies;
use crate::model::ValueUse;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    policies: &TypePolicies,
) -> Option<Vec<Type>> {
    let first = operands.first().map(|operand| &operand.ty);
    Some(match instruction.opcode.as_str() {
        "heap.alloc.counted" => vec![Type::Reference(Box::new(crate::text_types::explicit_type(
            source,
            instruction,
        )?))],
        "heap.alloc.unique" => vec![Type::Unique(Box::new(crate::text_types::explicit_type(
            source,
            instruction,
        )?))],
        "ref.addr" => vec![address_result(first?, true)?],
        "uniq.addr" => vec![address_result(first?, false)?],
        "heap.alloc.raw" => vec![Type::Pointer(AddressSpace::Generic)],
        "heap.realloc.raw" => vec![first?.clone()],
        "weak.new" => vec![weak_from_reference(first?)?],
        "weak.load" => {
            let Type::Weak(pointee) = first? else {
                return None;
            };
            vec![policies.optional_type(Type::Reference(pointee.clone()))]
        }
        "ref.unowned" => vec![Type::Pointer(AddressSpace::Generic)],
        "ref.isunique" => vec![Type::Integer(1)],
        _ => return None,
    })
}

pub(super) fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    operands: &[ValueUse],
    results: Option<&[Type]>,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let error =
        |message: String| Diagnostic::error(V2, message).with_primary_span(instruction.span);
    let result = results.and_then(|results| results.first());
    let valid = match instruction.opcode.as_str() {
        "heap.alloc.counted" | "heap.alloc.unique" => {
            let Some(allocated) = crate::text_types::explicit_type(source, instruction) else {
                return vec![error(format!(
                    "{} requires a valid type",
                    instruction.opcode
                ))];
            };
            let expected = if instruction.opcode == "heap.alloc.counted" {
                Type::Reference(Box::new(allocated))
            } else {
                Type::Unique(Box::new(allocated))
            };
            operands.is_empty() && instruction.results.len() == 1 && result == Some(&expected)
        }
        "ref.addr" | "uniq.addr" => {
            let expected = operands.first().and_then(|operand| {
                let pointee = match (&instruction.opcode[..], &operand.ty) {
                    ("ref.addr", Type::Reference(pointee))
                    | ("uniq.addr", Type::Unique(pointee)) => pointee.as_ref().clone(),
                    _ => return None,
                };
                Some(address_type(pointee))
            });
            operands.len() == 1 && instruction.results.len() == 1 && result == expected.as_ref()
        }
        "heap.alloc.raw" => {
            operands.len() == 2
                && operands
                    .iter()
                    .all(|operand| operand.ty == Type::Integer(64))
                && instruction.results.len() == 1
                && result == Some(&Type::Pointer(AddressSpace::Generic))
        }
        "heap.realloc.raw" => {
            operands.len() == 3
                && matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
                && operands[1..]
                    .iter()
                    .all(|operand| operand.ty == Type::Integer(64))
                && instruction.results.len() == 1
                && result == operands.first().map(|operand| &operand.ty)
        }
        "heap.free.raw" => {
            operands.len() == 1
                && matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Pointer(_))
                )
                && instruction.results.is_empty()
        }
        "weak.new" => {
            let expected = operands.first().and_then(|operand| match &operand.ty {
                Type::Reference(pointee) => Some(Type::Weak(pointee.clone())),
                _ => None,
            });
            operands.len() == 1 && instruction.results.len() == 1 && result == expected.as_ref()
        }
        "weak.load" => {
            let expected = operands.first().and_then(|operand| match &operand.ty {
                Type::Weak(pointee) => {
                    Some(policies.optional_type(Type::Reference(pointee.clone())))
                }
                _ => None,
            });
            operands.len() == 1 && instruction.results.len() == 1 && result == expected.as_ref()
        }
        "ref.unowned" => {
            operands.len() == 1
                && matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Reference(_))
                )
                && instruction.results.len() == 1
                && result == Some(&Type::Pointer(AddressSpace::Generic))
        }
        "ref.isunique" => {
            operands.len() == 1
                && matches!(
                    operands.first().map(|operand| &operand.ty),
                    Some(Type::Reference(_))
                )
                && instruction.results.len() == 1
                && result == Some(&Type::Integer(1))
        }
        _ => return Vec::new(),
    };
    if valid {
        Vec::new()
    } else {
        vec![error(format!(
            "{} has incompatible operands or result type",
            instruction.opcode
        ))]
    }
}

fn address_result(ty: &Type, reference: bool) -> Option<Type> {
    let pointee = match (reference, ty) {
        (true, Type::Reference(pointee)) | (false, Type::Unique(pointee)) => {
            pointee.as_ref().clone()
        }
        _ => return None,
    };
    Some(address_type(pointee))
}

fn address_type(pointee: Type) -> Type {
    Type::Address {
        pointee: Box::new(pointee),
        space: Some(AddressSpace::Generic),
    }
}

fn weak_from_reference(ty: &Type) -> Option<Type> {
    match ty {
        Type::Reference(pointee) => Some(Type::Weak(pointee.clone())),
        _ => None,
    }
}
