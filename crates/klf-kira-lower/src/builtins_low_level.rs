//! Wrapper names and types for the general low-level KIR builder services.
//!
//! These are the language-agnostic half of the service surface: arbitrary KIR
//! types, values, memory, calls, control flow and declarations, all addressed
//! by opaque handles.

use crate::builtins::parse_spec;
use crate::ty::FeType;

pub(crate) fn wrapper_types(service_name: &str) -> Option<(Vec<FeType>, FeType)> {
    Some(parse_spec(wrapper_spec(service_name)?))
}

/// The signature of a low-level wrapper in the compact spec notation of
/// `builtins::wrapper_spec`.
fn wrapper_spec(service_name: &str) -> Option<&'static str> {
    let spec = match service_name {
        "klf.source.virtual" => "sFqi->i",
        "klf.kir.evaluation.source" => "sFi->i",
        "klf.kir.module.detached" => "su->m",
        "klf.kir.module.evaluate" => "mfii->i",
        "klf.kir.module.evaluate_buffer" => "mfiiii->i",
        "klf.kir.module.evaluation_root" => "mf->-",
        "klf.kir.module.evaluator" => "m->i",
        "klf.kir.evaluator.evaluate_buffer" => "ifiiii->i",
        "klf.kir.evaluator.release" => "i->-",
        "klf.kir.evaluation.status" | "klf.kir.evaluation.value" | "klf.kir.evaluation.length" => {
            "i->i"
        }
        "klf.kir.evaluation.byte" => "ii->i",
        "klf.kir.type.integer" => "i->T",
        "klf.kir.type.address"
        | "klf.kir.type.reference"
        | "klf.kir.type.unique"
        | "klf.kir.type.function"
        | "klf.kir.type.foreign_function" => "T->T",
        "klf.kir.type.array" => "iT->T",
        "klf.kir.type.size" | "klf.kir.type.align" => "mT->i",
        "klf.kir.type.unit" | "klf.kir.type.pointer" => "->T",
        "klf.kir.type.enum_open" | "klf.kir.type.parameter" | "klf.kir.type.record_union" => "T->-",
        "klf.kir.type.record" | "klf.kir.type.enum_named" => "miii->T",
        "klf.kir.type.record_named" => "miiii->T",
        "klf.kir.type.record_field" | "klf.kir.type.enum_case" => "TiiT->-",
        "klf.kir.type.enum" => "mii->T",
        "klf.kir.type.enum_case_numbered" => "TiiiT->-",
        "klf.kir.value.enum_inject" => "fai->-",
        "klf.kir.function.define" => "miiT->f",
        "klf.kir.function.result"
        | "klf.kir.value.argument"
        | "klf.kir.value.call_indirect_void"
        | "klf.kir.value.heap_free_raw"
        | "klf.kir.block.else_argument" => "fv->-",
        "klf.kir.function.comptime_buffer" => "ffii->-",
        "klf.kir.function.comptime_array" => "ffiii->-",
        "klf.kir.function.referenced" => "f->i",
        "klf.kir.function.comptime"
        | "klf.kir.function.comptime_record"
        | "klf.kir.function.discard"
        | "klf.kir.function.result_void"
        | "klf.kir.function.body"
        | "klf.kir.function.initializer"
        | "klf.kir.block.unreachable" => "f->-",
        "klf.kir.function.parameter" => "fi->v",
        "klf.kir.value.integer" => "fTi->v",
        "klf.kir.value.float_literal" => "fTii->v",
        "klf.kir.value.float_binary" | "klf.kir.value.float_compare" | "klf.kir.value.compare" => {
            "fivv->v"
        }
        "klf.kir.value.float_negate" | "klf.kir.value.const_string_length" => "fv->v",
        "klf.kir.value.zero" | "klf.kir.value.null" => "fT->v",
        "klf.kir.value.text_byte" => "fi->-",
        "klf.kir.value.text" => "f->v",
        "klf.kir.value.const_string" => "fii->v",
        "klf.kir.value.heap_alloc_raw" => "fvv->v",
        "klf.kir.value.stack_alloc" => "fT->a",
        "klf.kir.value.store" => "fva->-",
        "klf.kir.value.mem_copy" | "klf.kir.value.mem_move" | "klf.kir.value.mem_set" => "fvvv->-",
        "klf.kir.value.load" | "klf.kir.value.from_address" | "klf.kir.value.enum_tag" => "fa->v",
        "klf.kir.value.field_address" | "klf.kir.value.enum_payload_address" => "faiT->a",
        "klf.kir.value.element_address" => "favT->a",
        "klf.kir.value.convert" => "fivT->v",
        "klf.kir.value.binary" => "fiiivv->v",
        "klf.kir.value.call" => "ffT->v",
        "klf.kir.value.call_void" => "ff->-",
        "klf.kir.value.function_reference" => "ff->v",
        "klf.kir.value.stack_dealloc" | "klf.kir.switch.enum" => "fa->-",
        "klf.kir.value.global_address" => "fg->a",
        "klf.kir.value.indirect_parameter" | "klf.kir.block.parameter" => "fT->-",
        "klf.kir.value.call_indirect" => "fvT->v",
        "klf.kir.address.from_value" => "fvT->a",
        "klf.kir.address.cast" => "faT->a",
        "klf.kir.signature.new" => "miiT->S",
        "klf.kir.signature.new_named" => "miiTi->S",
        "klf.kir.signature.name_byte" | "klf.kir.signature.c_syscall_byte" => "Si->-",
        "klf.kir.signature.parameter" => "ST->-",
        "klf.kir.signature.c_variadic" | "klf.kir.signature.export" => "S->-",
        "klf.kir.signature.c_parameter" | "klf.kir.signature.c_result" => "SiT->-",
        "klf.kir.module.attribute_byte" | "klf.kir.module.attribute" => "mi->-",
        "klf.kir.signature.define" | "klf.kir.signature.declare" => "S->f",
        "klf.kir.global.new" => "miiTi->g",
        "klf.kir.global.new_named" => "miiTii->g",
        "klf.kir.global.byte" | "klf.kir.global.integer" => "gi->-",
        "klf.kir.global.anonymous" => "mTi->g",
        "klf.kir.block.define" | "klf.kir.block.entry" => "f->b",
        "klf.kir.block.value" => "fbi->v",
        "klf.kir.block.switch" | "klf.kir.block.branch" => "fb->-",
        "klf.kir.block.conditional_branch" => "fvbb->-",
        "klf.kir.block.trap" | "klf.kir.function.at" => "fii->-",
        "klf.kir.block.trap_text_byte" => "fi->-",
        "klf.kir.switch.case" => "fib->-",
        "klf.kir.switch.int" => "fvb->-",
        _ => return machine_spec(service_name),
    };
    Some(spec)
}

/// The wrappers of the instructions close to the machine: `select`, `fma`,
/// the raw pointer instructions, the atomics, `fence`, the layout queries
/// and the vectors.
fn machine_spec(service_name: &str) -> Option<&'static str> {
    Some(match service_name {
        "klf.kir.value.select" | "klf.kir.value.fma" => "fvvv->v",
        "klf.kir.value.raw_load" => "fTvii->v",
        "klf.kir.value.raw_store" => "fvvii->-",
        "klf.kir.value.pointer_offset" | "klf.kir.value.pointer_diff" => "fvv->v",
        "klf.kir.value.atomic_load" => "fTvi->v",
        "klf.kir.value.atomic_store" => "fvvi->-",
        "klf.kir.value.atomic_rmw" => "fivvi->v",
        "klf.kir.value.compare_exchange" => "fivvvii->v",
        "klf.kir.value.compare_exchange_succeeded" => "fv->v",
        "klf.kir.value.fence" => "fi->-",
        "klf.kir.value.size_of" | "klf.kir.value.align_of" | "klf.kir.value.stride_of" => "fT->v",
        "klf.kir.type.vector" => "iT->T",
        "klf.kir.value.vector_splat" => "fTv->v",
        "klf.kir.value.vector_extract" => "fvi->v",
        "klf.kir.value.vector_shuffle_lane" => "fvi->-",
        "klf.kir.value.vector_insert" => "fviv->v",
        "klf.kir.value.vector_shuffle" => "fvvi->v",
        _ => return None,
    })
}
