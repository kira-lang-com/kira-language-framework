use tir_format::TextInstruction;
use tir_format::text::TokenKind;

use crate::calls::opcode_end;
use crate::instruction_types::{is_float_binary, is_float_unary, is_integer_binary};

pub(super) fn operand_count(instruction: &TextInstruction) -> usize {
    instruction.tokens[opcode_end(instruction)..]
        .windows(2)
        .filter(|window| {
            matches!(window[0].kind, TokenKind::Punctuation('%'))
                && matches!(window[1].kind, TokenKind::Atom(_))
        })
        .count()
}

pub(super) fn operand_arity(opcode: &str) -> Option<usize> {
    if is_integer_binary(opcode) {
        return Some(2);
    }
    if is_float_binary(opcode) {
        return Some(if matches!(opcode, "fma" | "fmuladd") {
            3
        } else {
            2
        });
    }
    if is_float_unary(opcode) {
        return Some(1);
    }
    match opcode {
        "gpu.builtin" | "gpu.spec_const" | "subgroup.elect" => Some(0),
        "gpu.barrier"
        | "gpu.memfence"
        | "frag.demote"
        | "frag.load_attachment"
        | "frag.raster_order.begin"
        | "frag.raster_order.end"
        | "frag.imageblock.addr"
        | "rt.get"
        | "rt.hitobject.miss"
        | "node.input.count"
        | "node.finished_cross_group"
        | "coop.length" => Some(0),
        "gpu.ddx"
        | "gpu.ddy"
        | "gpu.fwidth"
        | "quad.swap"
        | "quad.all"
        | "quad.any"
        | "subgroup.all"
        | "subgroup.any"
        | "subgroup.all_equal"
        | "subgroup.broadcast_first"
        | "subgroup.ballot"
        | "subgroup.inverse_ballot"
        | "subgroup.ballot_bit_count"
        | "subgroup.ballot_find_lsb"
        | "subgroup.ballot_find_msb"
        | "subgroup.reduce"
        | "subgroup.scan_inclusive"
        | "subgroup.scan_exclusive"
        | "rq.proceed"
        | "rq.commit_triangle"
        | "rq.abort"
        | "rq.get"
        | "rt.hitobject.from_query"
        | "rt.hitobject.get"
        | "tile.imageblock.addr"
        | "coop.convert"
        | "coop.reduce"
        | "tensor.query"
        | "node.input.addr"
        | "node.output.commit" => Some(1),
        "subgroup.broadcast"
        | "quad.broadcast"
        | "subgroup.shuffle"
        | "subgroup.shuffle_xor"
        | "subgroup.shuffle_up"
        | "subgroup.shuffle_down"
        | "subgroup.rotate"
        | "mesh.set_counts"
        | "mesh.write.vertex"
        | "mesh.write.primitive"
        | "mesh.write.indices"
        | "rq.commit_procedural"
        | "rt.call_callable"
        | "rt.hitobject.invoke"
        | "coop.extract"
        | "tensor.load"
        | "node.output.addr"
        | "node.output.allocate" => Some(2),
        "object.dispatch_mesh"
        | "rt.report_hit"
        | "coop.store"
        | "coop.insert"
        | "tensor.store" => Some(3),
        "rq.init" => Some(8),
        "rt.trace" => Some(11),
        "rt.hitobject.trace" => Some(10),
        "coop.fill" => Some(1),
        "coop.load" => Some(2),
        "icmp" | "fcmp" | "ptr.offset" | "ptr.diff" | "atomic.store" | "atomic.rmw"
        | "raw.store" | "mmio.store" | "code.store" | "va.copy" => Some(2),
        "select" | "fshl" | "fshr" | "atomic.cmpxchg" | "mem.copy" | "mem.move" | "mem.set" => {
            Some(3)
        }
        "ldexp" => Some(2),
        "trunc" | "zext" | "sext" | "fptrunc" | "fpext" | "sitofp" | "uitofp" | "fptosi"
        | "fptoui" | "ptrtoint" | "inttoptr" | "bitcast" | "fcvt" | "raw.load" | "atomic.load"
        | "prefetch" | "va.start" | "va.arg" | "va.end" | "assume" | "debug.value"
        | "debug.addr" | "stack.alloc.dyn" | "stack.restore" | "trap.if" | "code.load"
        | "code.addr" | "code.fromaddr" | "mmio.load" => Some(1),
        "const.int" | "const.float" | "const.bool" | "const.null" | "const.zero"
        | "const.poison" | "const.bytes" | "const.vec" | "const.sampler" | "func.ref"
        | "global.addr" | "tls.addr" | "block.addr" | "gpu.entry.ref" | "fp.flags.read"
        | "fp.flags.clear" | "fence" | "target.has" | "target.pagesize" | "slot.addr"
        | "slot.start" | "slot.end" | "stack.save" | "thread.pointer" | "frame.address"
        | "return.address" | "vscale.fixed" | "unreachable" | "trap" | "nop" | "debug.label"
        | "fence.io" | "kernel.percpu.addr" => Some(0),
        "foreign.addr" | "foreign.data" | "foreign.entry" => Some(0),
        _ => None,
    }
}

pub(super) fn result_arity(instruction: &TextInstruction) -> Option<usize> {
    let opcode = instruction.opcode.as_str();
    if let Some(expected) = crate::gpu_texture_types::result_arity(instruction) {
        return Some(expected);
    }
    if matches!(
        opcode,
        "gpu.barrier"
            | "gpu.memfence"
            | "frag.demote"
            | "frag.raster_order.begin"
            | "frag.raster_order.end"
            | "gpu.printf"
            | "object.dispatch_mesh"
            | "mesh.set_counts"
            | "mesh.write.vertex"
            | "mesh.write.primitive"
            | "mesh.write.indices"
            | "rq.init"
            | "rq.commit_triangle"
            | "rq.commit_procedural"
            | "rq.abort"
            | "rt.trace"
            | "rt.ignore_hit"
            | "rt.accept_and_end"
            | "rt.call_callable"
            | "rt.hitobject.invoke"
            | "rt.reorder"
            | "coop.store"
            | "tensor.store"
            | "vec.store"
            | "vec.scatter"
            | "node.output.commit"
    ) {
        return Some(0);
    }
    if matches!(opcode, "rq.get" | "rt.get" | "rt.hitobject.get")
        && matches!(
            ray_attribute(instruction),
            Some("object_to_world" | "world_to_object")
        )
    {
        return Some(12);
    }
    if opcode == "rt.get" && ray_attribute(instruction) == Some("triangle_vertex_positions") {
        return Some(9);
    }
    if matches!(
        opcode,
        "gpu.builtin"
            | "gpu.spec_const"
            | "gpu.ddx"
            | "gpu.ddy"
            | "gpu.fwidth"
            | "quad.broadcast"
            | "quad.swap"
            | "quad.all"
            | "quad.any"
            | "subgroup.elect"
            | "subgroup.all"
            | "subgroup.any"
            | "subgroup.all_equal"
            | "subgroup.broadcast"
            | "subgroup.broadcast_first"
            | "subgroup.ballot"
            | "subgroup.inverse_ballot"
            | "subgroup.ballot_bit_count"
            | "subgroup.ballot_find_lsb"
            | "subgroup.ballot_find_msb"
            | "subgroup.shuffle"
            | "subgroup.shuffle_xor"
            | "subgroup.shuffle_up"
            | "subgroup.shuffle_down"
            | "subgroup.rotate"
            | "subgroup.reduce"
            | "subgroup.scan_inclusive"
            | "subgroup.scan_exclusive"
            | "rq.proceed"
            | "rq.get"
            | "rt.report_hit"
            | "rt.get"
            | "rt.hitobject.trace"
            | "rt.hitobject.from_query"
            | "rt.hitobject.miss"
            | "rt.hitobject.get"
            | "coop.fill"
            | "coop.load"
            | "coop.mma"
            | "coop.convert"
            | "coop.length"
            | "coop.extract"
            | "coop.insert"
            | "coop.reduce"
            | "tensor.load"
            | "tensor.query"
            | "frag.imageblock.addr"
            | "tile.imageblock.addr"
            | "node.input.addr"
            | "node.input.count"
            | "node.output.allocate"
            | "node.output.addr"
            | "node.finished_cross_group"
            | "frag.load_attachment"
    ) {
        return Some(1);
    }
    if matches!(
        opcode,
        "add.ovf" | "sub.ovf" | "mul.ovf" | "frexp" | "landingpad" | "atomic.cmpxchg"
    ) {
        return Some(2);
    }
    if matches!(
        opcode,
        "const.int"
            | "const.float"
            | "const.bool"
            | "const.null"
            | "const.zero"
            | "const.bytes"
            | "const.vec"
            | "const.poison"
            | "const.sampler"
            | "func.ref"
            | "foreign.addr"
            | "foreign.data"
            | "foreign.entry"
            | "global.addr"
            | "tls.addr"
            | "block.addr"
            | "gpu.entry.ref"
            | "icmp"
            | "fcmp"
            | "fclass"
            | "fp.flags.read"
            | "setjmp"
            | "raw.load"
            | "atomic.load"
            | "mmio.load"
            | "atomic.rmw"
            | "ptr.offset"
            | "ptr.diff"
            | "stack.alloc.dyn"
            | "stack.save"
            | "slot.addr"
            | "code.load"
            | "code.addr"
            | "code.fromaddr"
            | "thread.pointer"
            | "frame.address"
            | "return.address"
            | "kernel.percpu.addr"
            | "vscale.fixed"
            | "vec.vscale"
            | "vec.splat"
            | "vec.extract"
            | "vec.insert"
            | "vec.shuffle"
            | "vec.step"
            | "vec.reduce"
            | "vec.load"
            | "vec.gather"
            | "vec.compress"
            | "vec.expand"
            | "vec.dot"
            | "vec.zip"
            | "vec.unzip"
            | "vec.reverse"
            | "vec.slide"
            | "vec.concat"
            | "vec.split"
            | "vec.mask.active"
            | "vec.mask.count"
            | "vec.mask.first"
            | "vec.table"
            | "va.arg"
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
            | "ctlz"
            | "cttz"
            | "ctpop"
            | "bswap"
            | "bitreverse"
    ) || is_integer_binary(opcode)
        || is_float_binary(opcode)
        || is_float_unary(opcode)
        || opcode.starts_with("math.")
        || matches!(
            opcode,
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
                | "select"
                | "freeze"
                | "ldexp"
                | "stack.alloc.dyn"
                | "target.has"
                | "target.pagesize"
        )
    {
        Some(1)
    } else if matches!(
        opcode,
        "raw.store"
            | "atomic.store"
            | "code.store"
            | "stack.restore"
            | "trap.if"
            | "mem.copy"
            | "mem.move"
            | "mem.set"
            | "prefetch"
            | "fence"
            | "mmio.store"
            | "fence.io"
            | "fp.flags.clear"
            | "slot.start"
            | "slot.end"
            | "va.start"
            | "va.copy"
            | "va.end"
            | "nop"
            | "assume"
            | "debug.value"
            | "debug.addr"
            | "debug.label"
            | "br"
            | "cond.br"
            | "switch.int"
            | "br.indirect"
            | "return"
            | "fail"
            | "tail.call"
            | "invoke"
            | "asm.goto"
            | "trap"
            | "unreachable"
    ) {
        Some(0)
    } else {
        None
    }
}

fn ray_attribute(instruction: &TextInstruction) -> Option<&str> {
    let start = opcode_end(instruction);
    if instruction.opcode == "rt.get" {
        return match &instruction.tokens.get(start)?.kind {
            TokenKind::Atom(attribute) => Some(attribute),
            _ => None,
        };
    }
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation(',')))?
        + start;
    match &instruction.tokens.get(comma + 1)?.kind {
        TokenKind::Atom(attribute) => Some(attribute),
        _ => None,
    }
}
