use std::collections::BTreeSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;
use tir_types::{FloatType, Type};
use tvisa_format::{BinaryOperand, TvisaImage};

use crate::profile::runtime_entry_id;
use tir_format::TextInstruction;

const VFY42: Code = Code::new(Dialect::Tvisa, Area::Verifier, 42);
const VFY50: Code = Code::new(Dialect::Tvisa, Area::Verifier, 50);

pub(super) fn verify_binary_section(image: &TvisaImage, memory_limit: u64) -> Vec<Diagnostic> {
    let identifiers = match image.runtime_entry_ids() {
        Ok(identifiers) => identifiers,
        Err(error) => return vec![error.diagnostic],
    };
    let mut diagnostics: Vec<_> = identifiers
        .iter()
        .copied()
        .filter(|identifier| !runtime_entry_exists(*identifier))
        .map(|identifier| {
            Diagnostic::error(
                VFY42,
                format!("runtime entry 0x{identifier:04X} is not defined"),
            )
        })
        .collect();
    let bodies = match image.function_bodies(memory_limit) {
        Ok(bodies) => bodies,
        Err(error) => return vec![error.diagnostic],
    };
    let used: BTreeSet<_> = bodies
        .iter()
        .flat_map(|body| &body.blocks)
        .flat_map(|block| &block.instructions)
        .filter(|instruction| instruction.opcode == 0x706)
        .filter_map(|instruction| {
            instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Index(id) => u16::try_from(*id).ok(),
                    _ => None,
                })
        })
        .collect();
    if identifiers.into_iter().collect::<BTreeSet<_>>() != used {
        diagnostics.push(Diagnostic::error(
            VFY50,
            "RUNTIME identifiers do not match call.runtime instructions",
        ));
    }
    diagnostics
}

pub(super) fn verify_function(body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            if instruction.opcode != "call.runtime" {
                continue;
            }
            let Some(id) = runtime_entry_id(instruction) else {
                diagnostics.push(
                    Diagnostic::error(VFY42, "call.runtime requires a 16-bit runtime entry ID")
                        .with_primary_span(instruction.span),
                );
                continue;
            };
            if !runtime_entry_exists(id) {
                diagnostics.push(
                    Diagnostic::error(VFY42, format!("runtime entry 0x{id:04X} is not defined"))
                        .with_primary_span(instruction.span),
                );
            }
        }
    }
    diagnostics
}

fn runtime_entry_exists(id: u16) -> bool {
    matches!(
        id,
        0x0001..=0x0011
            | 0x0020..=0x0021
            | 0x0030..=0x0033
            | 0x0040..=0x0041
            | 0x0050..=0x0053
            | 0x0060..=0x0065
            | 0x0070..=0x0082
            | 0x0088..=0x0097
            | 0x00A0
            | 0x00A8
            | 0x00B0
            | 0x0170..=0x0182
            | 0xF000..=0xFFFF
    ) || math_runtime_entry(id, 0x0800)
        || math_runtime_entry(id, 0x0880)
}

fn math_runtime_entry(id: u16, base: u16) -> bool {
    let Some(offset) = id.checked_sub(base) else {
        return false;
    };
    let operation = offset / 4;
    let float_type = offset % 4;
    operation <= 24 && float_type <= 2
}

pub(super) fn runtime_call_signature(
    instruction: &TextInstruction,
) -> Option<(Vec<Type>, Vec<Type>)> {
    let id = runtime_entry_id(instruction)?;
    runtime_call_signature_by_id(id)
}

pub(super) fn runtime_call_signature_by_id(id: u16) -> Option<(Vec<Type>, Vec<Type>)> {
    let pointer = Type::Pointer(tir_types::AddressSpace::Generic);
    let i64_type = Type::Integer(64);
    let i32_type = Type::Integer(32);
    let i1_type = Type::Integer(1);
    let none = Vec::new();
    let shape = match id {
        0x0001 => (
            vec![i64_type.clone(), i64_type.clone(), i64_type.clone()],
            vec![pointer.clone()],
        ),
        0x0002 | 0x0003 => (
            vec![i64_type.clone(), i64_type.clone(), pointer.clone()],
            vec![pointer.clone()],
        ),
        0x0004 => (
            vec![pointer.clone(), i64_type.clone(), i64_type.clone()],
            vec![pointer.clone()],
        ),
        0x0005 | 0x0007 | 0x000D | 0x000F => (vec![pointer.clone()], none.clone()),
        0x0006 | 0x000B | 0x000C => (vec![pointer.clone()], vec![pointer.clone()]),
        0x0008 => (
            vec![pointer.clone(), i64_type.clone()],
            vec![pointer.clone()],
        ),
        0x0009 => (vec![pointer.clone(), i64_type.clone()], none.clone()),
        0x000A => (vec![pointer.clone()], vec![i64_type.clone()]),
        0x000E => (vec![pointer.clone(), pointer.clone()], none.clone()),
        0x0010 => (
            vec![pointer.clone(), i64_type.clone(), pointer.clone()],
            none.clone(),
        ),
        0x0011 => (vec![pointer.clone()], none.clone()),
        0x0020 => (
            vec![i64_type.clone(), i64_type.clone()],
            vec![pointer.clone()],
        ),
        0x0021 => (
            vec![pointer.clone(), i64_type.clone(), i64_type.clone()],
            none.clone(),
        ),
        0x0030 => (vec![pointer.clone(), i64_type.clone()], none.clone()),
        0x0031 | 0x0033 => (vec![pointer.clone()], none.clone()),
        0x0032 => (vec![pointer.clone()], vec![pointer.clone()]),
        0x0040 => (vec![pointer.clone()], vec![i64_type.clone()]),
        0x0041 => (vec![pointer.clone(), i64_type.clone()], none.clone()),
        0x0050 | 0x0051 | 0x0053 => (
            vec![pointer.clone(), pointer.clone(), i64_type.clone()],
            none.clone(),
        ),
        0x0052 => (
            vec![pointer.clone(), i64_type.clone(), i64_type.clone()],
            none.clone(),
        ),
        0x0060..=0x0065 => (
            vec![
                i64_type.clone(),
                i64_type.clone(),
                i64_type.clone(),
                i64_type.clone(),
            ],
            vec![i64_type.clone(), i64_type.clone()],
        ),
        0x0070..=0x0082 | 0x0170..=0x0182 => {
            let base = if id >= 0x0100 { id - 0x0100 } else { id };
            f128_runtime_signature(base, &i64_type, &i32_type)
        }
        0x0088..=0x0097 => conversion_runtime_signature(id, &i64_type, &i32_type),
        0x00A0 => (
            vec![
                i64_type.clone(),
                pointer.clone(),
                i64_type.clone(),
                i64_type.clone(),
                i64_type.clone(),
                i64_type.clone(),
            ],
            vec![i64_type.clone(), i64_type.clone(), i1_type],
        ),
        0x00A8 => (
            vec![Type::Code, pointer.clone(), pointer.clone()],
            none.clone(),
        ),
        0x00B0 => (none.clone(), none.clone()),
        0x0800..=0x0862 | 0x0880..=0x08E2 => math_runtime_signature(id, &i64_type),
        _ => return None,
    };
    Some(shape)
}

fn f128_runtime_signature(base: u16, i64_type: &Type, i32_type: &Type) -> (Vec<Type>, Vec<Type>) {
    let pair = vec![i64_type.clone(), i64_type.clone()];
    match base {
        0x0070..=0x0073 | 0x0077..=0x007B => (repeated_pair(i64_type, 2), pair.clone()),
        0x0074 | 0x007C..=0x0080 => (pair.clone(), pair),
        0x0075 => (repeated_pair(i64_type, 3), pair),
        0x0076 => (repeated_pair(i64_type, 2), vec![i32_type.clone()]),
        0x0081 => {
            let mut args = pair;
            args.push(i32_type.clone());
            (args, vec![i64_type.clone(), i64_type.clone()])
        }
        0x0082 => {
            let mut results = pair;
            results.push(i32_type.clone());
            (vec![i64_type.clone(), i64_type.clone()], results)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

fn repeated_pair(ty: &Type, count: usize) -> Vec<Type> {
    (0..count).flat_map(|_| [ty.clone(), ty.clone()]).collect()
}

fn conversion_runtime_signature(
    id: u16,
    i64_type: &Type,
    i32_type: &Type,
) -> (Vec<Type>, Vec<Type>) {
    let pair = vec![i64_type.clone(), i64_type.clone()];
    let f32_type = Type::Float(FloatType::F32);
    let f64_type = Type::Float(FloatType::F64);
    let f16_type = Type::Float(FloatType::F16);
    let bf16_type = Type::Float(FloatType::Bf16);
    match id {
        0x0088 | 0x0089 => (vec![i64_type.clone()], pair),
        0x008A | 0x008B => (pair.clone(), pair),
        0x008C | 0x008D => (
            vec![i64_type.clone(), i64_type.clone(), i32_type.clone()],
            vec![i64_type.clone()],
        ),
        0x008E | 0x008F => (
            vec![i64_type.clone(), i64_type.clone(), i32_type.clone()],
            pair,
        ),
        0x0090 => (vec![f32_type.clone()], pair.clone()),
        0x0091 => (vec![f64_type.clone()], pair.clone()),
        0x0092 => (pair.clone(), vec![f32_type]),
        0x0093 => (pair.clone(), vec![f64_type]),
        0x0094 => (vec![f16_type.clone()], pair.clone()),
        0x0095 => (pair.clone(), vec![f16_type]),
        0x0096 => (vec![bf16_type.clone()], pair),
        0x0097 => (pair, vec![bf16_type]),
        _ => (Vec::new(), Vec::new()),
    }
}

fn math_runtime_signature(id: u16, i64_type: &Type) -> (Vec<Type>, Vec<Type>) {
    let base = if id >= 0x0880 { id - 0x0080 } else { id };
    let offset = base - 0x0800;
    let operation = offset / 4;
    let float_kind = offset % 4;
    if operation > 24 || float_kind > 2 {
        return (Vec::new(), Vec::new());
    }
    let arity = if matches!(operation, 6 | 21 | 23) {
        2
    } else {
        1
    };
    let float = match float_kind {
        0 => FloatType::F32,
        1 => FloatType::F64,
        _ => FloatType::F128,
    };
    if float == FloatType::F128 {
        let args = vec![i64_type.clone(); arity * 2];
        (args, vec![i64_type.clone(), i64_type.clone()])
    } else {
        let ty = Type::Float(float);
        (vec![ty.clone(); arity], vec![ty])
    }
}
