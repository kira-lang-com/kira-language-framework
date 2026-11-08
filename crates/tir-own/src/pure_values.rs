use tir_types::Type;

pub(super) fn is_pure_value_type(ty: &Type) -> bool {
    match ty {
        Type::Integer(_) | Type::Float(_) | Type::Pointer(_) | Type::Unit | Type::Never => true,
        Type::Tuple(elements) => elements.iter().all(is_pure_value_type),
        Type::Array { count, element } => *count <= 1024 && is_pure_value_type(element),
        Type::FixedVector { lanes, element } => {
            *lanes > 0 && matches!(element.as_ref(), Type::Integer(_) | Type::Float(_))
        }
        _ => false,
    }
}

pub(super) fn is_pure_value_instruction(op: &str) -> bool {
    if matches!(
        op,
        "tuple.new" | "tuple.extract" | "array.new" | "array.extract" | "array.insert"
    ) {
        return true;
    }
    if op.starts_with("vec.") {
        return true;
    }
    if matches!(
        op,
        "trunc"
            | "zext"
            | "sext"
            | "fptrunc"
            | "fpext"
            | "sitofp"
            | "uitofp"
            | "fptosi"
            | "fptoui"
            | "ptrtoint"
            | "inttoptr"
            | "bitcast"
            | "fcvt"
    ) {
        return true;
    }
    if is_pure_value_float_operation(op) {
        return true;
    }
    matches!(
        op,
        "return"
            | "br"
            | "cond.br"
            | "const.int"
            | "const.bool"
            | "const.float"
            | "const.zero"
            | "const.null"
            | "const.poison"
            | "const.bytes"
            | "const.aggregate"
            | "const.sampler"
            | "const.string"
            | "add"
            | "sub"
            | "mul"
            | "sdiv"
            | "udiv"
            | "srem"
            | "urem"
            | "and"
            | "or"
            | "xor"
            | "not"
            | "neg"
            | "abs"
            | "shl"
            | "lshr"
            | "ashr"
            | "rotl"
            | "rotr"
            | "fshl"
            | "fshr"
            | "smin"
            | "smax"
            | "umin"
            | "umax"
            | "ctlz"
            | "cttz"
            | "ctpop"
            | "bswap"
            | "bitreverse"
            | "clmul"
            | "clmulh"
            | "icmp"
            | "select"
            | "freeze"
            | "copy"
            | "move"
            | "destroy"
            | "borrow.begin"
            | "borrow.end"
    )
}

fn is_pure_value_float_operation(op: &str) -> bool {
    if op.starts_with("math.") {
        return true;
    }
    matches!(
        op,
        "fadd"
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
    )
}
