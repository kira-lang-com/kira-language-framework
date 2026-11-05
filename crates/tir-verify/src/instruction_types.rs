use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_types::{Abi, AddressSpace, Type};

use crate::Instruction;

#[path = "instruction_type_checks.rs"]
mod instruction_type_checks;
use instruction_type_checks::*;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_instruction(instruction: &Instruction) -> Vec<Diagnostic> {
    if let Some(diagnostics) = crate::conversion_types::verify_instruction(instruction) {
        return diagnostics;
    }
    let mut diagnostics = Vec::new();
    let operands = instruction
        .operands
        .iter()
        .map(|operand| (&operand.ty, operand.span))
        .collect::<Vec<_>>();
    let results = instruction
        .results
        .iter()
        .map(|result| (&result.ty, result.span))
        .collect::<Vec<_>>();

    match instruction.opcode.as_str() {
        "const.int" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_integer_scalar,
            &mut diagnostics,
        ),
        "const.float" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_float_scalar,
            &mut diagnostics,
        ),
        "const.bool" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_i1,
            &mut diagnostics,
        ),
        "const.null" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_pointer,
            &mut diagnostics,
        ),
        "const.zero" | "const.bytes" | "const.aggregate" => {
            verify_constant(
                &instruction.opcode,
                &operands,
                &results,
                |_| true,
                &mut diagnostics,
            );
        }
        "const.poison" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |_| true,
            &mut diagnostics,
        ),
        "const.string" => {
            if require_counts(
                &instruction.opcode,
                &operands,
                &results,
                0,
                1,
                &mut diagnostics,
            ) {
                let expected = Type::Tuple(vec![
                    Type::Pointer(tir_types::AddressSpace::Generic),
                    Type::Integer(64),
                ]);
                if results[0].0 != &expected {
                    report(
                        &mut diagnostics,
                        results[0].1,
                        format!(
                            "const.string result has type {}; expected {expected}",
                            results[0].0
                        ),
                    );
                }
            }
        }
        "global.addr" | "tls.addr" | "descriptor.ref" | "witness.ref" | "block.addr"
        | "kernel.percpu.addr" => {
            verify_constant(
                &instruction.opcode,
                &operands,
                &results,
                is_pointer,
                &mut diagnostics,
            );
        }
        "foreign.addr" | "foreign.entry" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Function(signature) if signature.foreign && signature.abi == Abi::C),
            &mut diagnostics,
        ),
        "foreign.data" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Pointer(AddressSpace::Generic)),
            &mut diagnostics,
        ),
        "const.sampler" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Sampler),
            &mut diagnostics,
        ),
        "const.vec" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::FixedVector { .. }),
            &mut diagnostics,
        ),
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "srem" | "urem" | "and" | "or" | "xor"
        | "shl" | "lshr" | "ashr" | "rotl" | "rotr" | "smin" | "smax" | "umin" | "umax"
        | "mulh" | "clmul" | "clmulh" => {
            verify_binary(
                instruction,
                &operands,
                &results,
                is_integer,
                &mut diagnostics,
            );
            if matches!(instruction.opcode.as_str(), "clmul" | "clmulh")
                && operands.first().is_some_and(|(ty, _)| {
                    integer_element_width(ty).is_some_and(|width| !matches!(width, 32 | 64))
                })
            {
                report(
                    &mut diagnostics,
                    instruction.span,
                    "clmul operands must be i32 or i64",
                );
            }
        }
        "add.ovf" | "sub.ovf" | "mul.ovf" => {
            verify_overflow_binary(instruction, &operands, &results, &mut diagnostics);
        }
        "fshl" | "fshr" => verify_binary_ternary(
            instruction,
            &operands,
            &results,
            is_integer,
            &mut diagnostics,
        ),
        "icmp" => verify_compare(
            instruction,
            &operands,
            &results,
            is_integer_or_pointer,
            &mut diagnostics,
        ),
        "fcmp" => verify_compare(instruction, &operands, &results, is_float, &mut diagnostics),
        "select" => verify_select(instruction, &operands, &results, &mut diagnostics),
        "closure.new" => verify_closure_new(&operands, &results, &mut diagnostics),
        "closure.callback" => {
            require_counts(
                "closure.callback",
                &operands,
                &results,
                0,
                1,
                &mut diagnostics,
            );
        }
        "not" | "neg" | "abs" | "ctlz" | "cttz" | "ctpop" | "bswap" | "bitreverse" => {
            verify_unary(
                instruction,
                &operands,
                &results,
                is_integer,
                &mut diagnostics,
            );
            if instruction.opcode == "bswap"
                && operands.first().is_some_and(|(ty, _)| {
                    integer_element_width(ty).is_some_and(|width| width % 16 != 0)
                })
            {
                report(
                    &mut diagnostics,
                    instruction.span,
                    "bswap requires an integer width divisible by 16",
                );
            }
        }
        "freeze" => verify_unary(instruction, &operands, &results, |_| true, &mut diagnostics),
        "fneg" | "fabs" | "fsqrt" | "ffloor" | "fceil" | "ftrunc" | "fround" | "froundeven" => {
            verify_unary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        "fadd" | "fsub" | "fmul" | "fdiv" | "frem" | "fmin" | "fmax" | "fminnum" | "fmaxnum"
        | "fcopysign" => {
            verify_binary(instruction, &operands, &results, is_float, &mut diagnostics);
        }
        "fma" | "fmuladd" => {
            verify_ternary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        "fclass" => verify_fclass(instruction, &operands, &results, &mut diagnostics),
        "frexp" => verify_frexp(instruction, &operands, &results, &mut diagnostics),
        "ldexp" => verify_ldexp(instruction, &operands, &results, &mut diagnostics),
        "fp.flags.read" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Integer(32)),
            &mut diagnostics,
        ),
        "fp.flags.clear" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Unit),
            &mut diagnostics,
        ),
        "math.atan2" | "math.pow" | "math.hypot" => {
            verify_binary(instruction, &operands, &results, is_float, &mut diagnostics);
        }
        "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan"
        | "math.sinh" | "math.cosh" | "math.tanh" | "math.asinh" | "math.acosh" | "math.atanh"
        | "math.exp" | "math.exp2" | "math.exp10" | "math.expm1" | "math.log" | "math.log2"
        | "math.log10" | "math.log1p" | "math.cbrt" | "math.rsqrt" => {
            verify_unary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        _ => {}
    }
    diagnostics
}
