use super::{is_conversion, is_operandless_definition};

pub(super) fn expected_operand_count(opcode: &str) -> Option<usize> {
    if let Some(count) = crate::gpu_profile::instructions::expected_operand_count(opcode) {
        return Some(count);
    }
    if is_operandless_definition(opcode) || matches!(opcode, "fp.flags.read" | "fp.flags.clear") {
        return Some(0);
    }
    if is_conversion(opcode)
        || matches!(
            opcode,
            "field.addr"
                | "stack.dealloc"
                | "enum.payload.addr"
                | "addr.cast"
                | "addr.toptr"
                | "ptr.toaddr"
                | "load"
                | "load.borrow"
                | "borrow.begin"
        )
        || matches!(
            opcode,
            "not"
                | "neg"
                | "abs"
                | "ctlz"
                | "cttz"
                | "ctpop"
                | "bswap"
                | "bitreverse"
                | "freeze"
                | "fneg"
                | "fabs"
                | "fsqrt"
                | "ffloor"
                | "fceil"
                | "ftrunc"
                | "fround"
                | "froundeven"
                | "fclass"
                | "frexp"
                | "math.sin"
                | "math.cos"
                | "math.tan"
                | "math.asin"
                | "math.acos"
                | "math.atan"
                | "math.sinh"
                | "math.cosh"
                | "math.tanh"
                | "math.asinh"
                | "math.acosh"
                | "math.atanh"
                | "math.exp"
                | "math.exp2"
                | "math.exp10"
                | "math.expm1"
                | "math.log"
                | "math.log2"
                | "math.log10"
                | "math.log1p"
                | "math.cbrt"
                | "math.rsqrt"
        )
    {
        return Some(1);
    }
    if opcode == "elem.addr" {
        return Some(2);
    }
    if matches!(
        opcode,
        "struct.extract"
            | "struct.destructure"
            | "tuple.extract"
            | "tuple.destructure"
            | "enum.tag"
            | "enum.take"
            | "enum.project"
    ) {
        return Some(1);
    }
    if matches!(opcode, "struct.insert" | "array.extract") {
        return Some(2);
    }
    if opcode == "array.insert" {
        return Some(3);
    }
    if matches!(opcode, "fma" | "fmuladd" | "fshl" | "fshr" | "select") {
        return Some(3);
    }
    if matches!(
        opcode,
        "add"
            | "sub"
            | "mul"
            | "sdiv"
            | "udiv"
            | "srem"
            | "urem"
            | "and"
            | "or"
            | "xor"
            | "shl"
            | "lshr"
            | "ashr"
            | "rotl"
            | "rotr"
            | "smin"
            | "smax"
            | "umin"
            | "umax"
            | "mulh"
            | "clmul"
            | "clmulh"
            | "add.ovf"
            | "sub.ovf"
            | "mul.ovf"
            | "icmp"
            | "fcmp"
            | "fadd"
            | "fsub"
            | "fmul"
            | "fdiv"
            | "frem"
            | "fmin"
            | "fmax"
            | "fminnum"
            | "fmaxnum"
            | "fcopysign"
            | "ldexp"
            | "math.atan2"
            | "math.pow"
            | "math.hypot"
    ) {
        return Some(2);
    }
    None
}
