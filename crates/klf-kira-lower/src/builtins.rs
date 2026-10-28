//! Built-in kernel service calls exposed to KLF Kira (section 8.12 / 15.7).
//! `putc`, `puti`, `source_len` and `source_byte` lower to `call.foreign`
//! services. The VM resolves them against its host-capability table and a
//! native build links them from `klf-native-rt`. This is how definition code
//! reaches kernel services instead of being hosted in Rust.
#[path = "builtins_definition.rs"]
mod definition;
#[path = "builtins_shader.rs"]
mod shader;

use crate::Signature;
use crate::ty::{DefinitionHandle, FeType};
use klf_core::{FileId, Name};
use klf_kir::{Engine, Function, Linkage, Parameter, Type};
use klf_services::{KernelService, ServiceType, Session};
use klf_source::Span;
/// The wrapper symbol a service is called through.
///
/// The symbol follows from the service name: drop the `klf.` prefix and turn
/// the remaining dots into underscores. The two `klf.io.*` wrappers predate
/// that rule and keep their shorter spellings.
fn wrapper_name(service_name: &str) -> Option<String> {
    let suffix = service_name.strip_prefix("klf.")?;
    wrapper_types_of(service_name)?;
    let name = match service_name {
        "klf.io.put" => "putc".to_owned(),
        "klf.io.puti" => "puti".to_owned(),
        _ => suffix.replace('.', "_"),
    };
    Some(name)
}
/// Returns a manifest entry for a KLF Kira service wrapper.
#[must_use]
pub fn service<'a>(session: &'a Session, name: &str) -> Option<&'a KernelService> {
    session
        .kernel_services()
        .services()
        .iter()
        .find(|service| wrapper_name(&service.name).as_deref() == Some(name))
}
/// The built-in signatures, seeded into name resolution so a service call type-
/// checks like any other call.
#[must_use]
pub fn signatures(session: &Session) -> Vec<(Name, Signature)> {
    let mut signatures: Vec<(Name, Signature)> = session
        .kernel_services()
        .services()
        .iter()
        .filter_map(|service| {
            let wrapper = wrapper_name(&service.name)?;
            let (params, result) = wrapper_types(service)?;
            Some((Name::new(wrapper), Signature::free(params, result)))
        })
        .collect();
    signatures.push((
        Name::new("floatToBits"),
        Signature::free(vec![FeType::Float { bits: 64 }], signed_i64()),
    ));
    signatures.push((
        Name::new("bitsToFloat"),
        Signature::free(vec![signed_i64()], FeType::Float { bits: 64 }),
    ));
    signatures
}
/// The foreign declarations the service calls resolve against, added to every
/// lowered module.
#[must_use]
pub fn foreign_declarations(session: &Session) -> Vec<Function> {
    session
        .kernel_services()
        .services()
        .iter()
        .map(|service| {
            let parameters = service
                .signature
                .parameters
                .iter()
                .enumerate()
                .map(|(index, ty)| (format!("arg{index}"), kir_type(*ty)))
                .collect();
            foreign(
                &service.symbol,
                parameters,
                kir_type(service.signature.result),
            )
        })
        .collect()
}
/// The signature of a wrapper, written as a compact spec: one letter per
/// parameter, then `->`, then the result letter.
///
/// The letters are `i` for `i64`, `8` for `u8`, `-` for void, and one letter
/// per definition handle: `s` session, `F` source file, `u` unit, `p` program,
/// `m` module
/// builder, `M` finished module, `t` type builder, `T` type, `S` signature,
/// `g` global, `f` function builder, `b` block builder, `v` value, `a`
/// address, `x` a `String` the service reads as bytes and a length, `k` a
/// shader module.
/// Every wrapper signature, as `(spec, the services that have it)`.
static WRAPPER_SPECS: &[(&str, &[&str])] = &[
    ("i->i", &["klf.io.put"]),
    ("i->-", &["klf.io.puti"]),
    ("->i", &["klf.source.len"]),
    ("i->8", &["klf.source.byte"]),
    ("sF->i", &["klf.source.file_len"]),
    ("sFi->8", &["klf.source.file_byte"]),
    ("sF->i", &["klf.source.file_name_len"]),
    ("sFi->8", &["klf.source.file_name_byte"]),
    ("->i", &["klf.source.name_len"]),
    ("i->8", &["klf.source.name_byte"]),
    ("ixx->-", &["klf.test.expect"]),
    ("xx->-", &["klf.test.source"]),
    ("xx->i", &["klf.test.formats"]),
    ("->i", &["klf.test.run", "klf.test.compile"]),
    ("iii->i", &["klf.test.diagnosed"]),
    ("x->i", &["klf.test.reported", "klf.test.source_path"]),
    (
        "qi->i",
        &[
            "klf.source.named_exists",
            "klf.source.named_len",
            "klf.source.copy",
            "klf.source.name_copy",
        ],
    ),
    ("qii->i", &["klf.source.named_byte"]),
    (
        "sFqi->i",
        &["klf.source.file_copy", "klf.source.file_name_copy"],
    ),
    ("qiqi->i", &["klf.source.named_copy"]),
    ("si->i", &["klf.source.directory_file"]),
    (
        "sqi->i",
        &[
            "klf.source.read",
            "klf.source.path_exists",
            "klf.session.std_root_copy",
            "klf.session.target_copy",
        ],
    ),
    (
        "s->i",
        &["klf.session.std_root_len", "klf.session.target_len"],
    ),
    ("sqiqi->i", &["klf.source.directory"]),
    ("sFi->i", &["klf.source.digest"]),
    ("sFx->i", &["klf.subquery.recall"]),
    ("sxF->-", &["klf.subquery.keep"]),
    ("sFFiix->-", &["klf.source.expansion"]),
    ("->i", &["klf.editor.mode"]),
    (
        "iiiii->i",
        &[
            "klf.editor.classify",
            "klf.editor.reference",
            "klf.editor.call_edge",
        ],
    ),
    ("iqi->i", &["klf.editor.classify_many"]),
    ("iiiiiiii->i", &["klf.editor.symbol"]),
    (
        "ii->i",
        &["klf.editor.symbol_flags", "klf.editor.type_edge"],
    ),
    ("iiiiii->i", &["klf.editor.declaration"]),
    (
        "iii->i",
        &[
            "klf.editor.unresolved",
            "klf.editor.node",
            "klf.editor.inactive",
        ],
    ),
    ("iiii->i", &["klf.editor.scope", "klf.editor.fold"]),
    ("iiqi->i", &["klf.editor.symbol_text"]),
    ("iiiqi->i", &["klf.editor.value_at"]),
    ("iiiiqi->i", &["klf.editor.type_at", "klf.editor.runnable"]),
    ("iiiiiqi->i", &["klf.editor.hint", "klf.editor.link"]),
    ("x->i", &["klf.format.text"]),
    ("ii->i", &["klf.format.token", "klf.format.verbatim"]),
    ("i->i", &["klf.format.line"]),
    (
        "->i",
        &["klf.format.group", "klf.format.indent", "klf.format.end"],
    ),
    ("Fiii->-", &["klf.diag.error_in"]),
    ("Fiiiii->-", &["klf.diag.error_subject"]),
    ("iFiix->-", &["klf.diag.detail"]),
    ("iii->-", &["klf.diag.error"]),
    ("->-", &["klf.diag.hold"]),
    ("i->i", &["klf.diag.release"]),
    ("iiii->-", &["klf.diag.error_origin"]),
    ("sup->m", &["klf.kir.module.new"]),
    ("i->T", &["klf.kir.type.float"]),
    (
        "mii->t",
        &["klf.kir.type.enum_new", "klf.kir.type.struct_new"],
    ),
    (
        "tii->-",
        &[
            "klf.kir.type.enum_variant",
            "klf.kir.type.enum_variant_i64",
            "klf.kir.type.enum_variant_i32",
            "klf.kir.type.struct_field_i64",
            "klf.kir.type.struct_field_i32",
        ],
    ),
    (
        "ttii->-",
        &[
            "klf.kir.type.enum_variant_struct",
            "klf.kir.type.struct_field_enum",
            "klf.kir.type.struct_field_struct",
        ],
    ),
    (
        "tiii->-",
        &[
            "klf.kir.type.enum_variant_array_i64",
            "klf.kir.type.enum_variant_array_i32",
            "klf.kir.type.struct_field_array_i64",
            "klf.kir.type.struct_field_array_i32",
        ],
    ),
    (
        "mii->f",
        &["klf.kir.function.new", "klf.kir.function.new_void"],
    ),
    ("fi->-", &["klf.kir.function.return_int"]),
    (
        "f->-",
        &[
            "klf.kir.function.return_void",
            "klf.kir.function.unreachable",
        ],
    ),
    ("fii->-", &["klf.kir.function.trap"]),
    ("m->M", &["klf.kir.module.finish"]),
    (
        "fi->v",
        &[
            "klf.kir.value.const_int",
            "klf.kir.function.param_i64",
            "klf.kir.function.param_i32",
            "klf.kir.function.param_f64",
            "klf.kir.function.param_pair_i64",
            "klf.kir.function.param_pair_i32",
            "klf.kir.function.param_struct",
            "klf.kir.function.param_enum",
            "klf.kir.function.param_array_i64",
            "klf.kir.function.param_array_i32",
            "klf.kir.value.array_i64",
            "klf.kir.value.array_i32",
            "klf.kir.value.const_bool",
            "klf.kir.value.const_i32",
            "klf.kir.value.const_f64",
        ],
    ),
    ("fii->v", &["klf.kir.value.const_string"]),
    ("fv->v", &["klf.kir.value.const_string_length"]),
    (
        "fvv->v",
        &[
            "klf.kir.value.add",
            "klf.kir.value.pair_i64",
            "klf.kir.value.pair_i32",
            "klf.kir.value.add_i32",
            "klf.kir.value.sub_i64",
            "klf.kir.value.mul_i64",
            "klf.kir.value.div_i64",
            "klf.kir.value.rem_i64",
            "klf.kir.value.and_i64",
            "klf.kir.value.or_i64",
            "klf.kir.value.xor_i64",
            "klf.kir.value.shl_i64",
            "klf.kir.value.lshr_i64",
            "klf.kir.value.ashr_i64",
            "klf.kir.value.sub_i32",
            "klf.kir.value.mul_i32",
            "klf.kir.value.div_i32",
            "klf.kir.value.rem_i32",
            "klf.kir.value.and_i32",
            "klf.kir.value.or_i32",
            "klf.kir.value.xor_i32",
            "klf.kir.value.shl_i32",
            "klf.kir.value.lshr_i32",
            "klf.kir.value.ashr_i32",
            "klf.kir.value.eq_i64",
            "klf.kir.value.ne_i64",
            "klf.kir.value.lt_i64",
            "klf.kir.value.le_i64",
            "klf.kir.value.gt_i64",
            "klf.kir.value.ge_i64",
            "klf.kir.value.eq_i32",
            "klf.kir.value.ne_i32",
            "klf.kir.value.lt_i32",
            "klf.kir.value.le_i32",
            "klf.kir.value.gt_i32",
            "klf.kir.value.ge_i32",
            "klf.kir.value.add_f64",
            "klf.kir.value.sub_f64",
            "klf.kir.value.mul_f64",
            "klf.kir.value.div_f64",
            "klf.kir.value.eq_f64",
            "klf.kir.value.ne_f64",
            "klf.kir.value.lt_f64",
            "klf.kir.value.le_f64",
            "klf.kir.value.gt_f64",
            "klf.kir.value.ge_f64",
        ],
    ),
    ("fti->v", &["klf.kir.value.enum_new"]),
    (
        "ftiv->v",
        &[
            "klf.kir.value.enum_new_i64",
            "klf.kir.value.enum_new_i32",
            "klf.kir.value.enum_new_struct",
            "klf.kir.value.enum_new_array_i64",
            "klf.kir.value.enum_new_array_i32",
        ],
    ),
    (
        "fvi->v",
        &[
            "klf.kir.value.enum_payload_i64",
            "klf.kir.value.enum_payload_i32",
            "klf.kir.value.enum_payload_struct",
            "klf.kir.value.enum_payload_array_i64",
            "klf.kir.value.enum_payload_array_i32",
            "klf.kir.value.struct_get_i64",
            "klf.kir.value.struct_get_i32",
            "klf.kir.value.struct_get_enum",
            "klf.kir.value.struct_get_struct",
            "klf.kir.value.struct_get_array_i64",
            "klf.kir.value.struct_get_array_i32",
            "klf.kir.value.pair_get_i64",
            "klf.kir.value.pair_get_i32",
            "klf.kir.value.array_get_i64",
            "klf.kir.value.array_get_i32",
        ],
    ),
    ("ft->v", &["klf.kir.value.struct_new"]),
    (
        "fviv->-",
        &[
            "klf.kir.value.struct_set_i64",
            "klf.kir.value.struct_set_i32",
            "klf.kir.value.struct_set_enum",
            "klf.kir.value.struct_set_struct",
            "klf.kir.value.struct_set_array_i64",
            "klf.kir.value.struct_set_array_i32",
            "klf.kir.value.array_set_i64",
            "klf.kir.value.array_set_i32",
        ],
    ),
    (
        "fv->v",
        &[
            "klf.kir.value.enum_tag_i64",
            "klf.kir.value.enum_tag_i32",
            "klf.kir.value.i32_to_i64",
            "klf.kir.value.i64_to_i32",
            "klf.kir.value.i64_to_f64",
            "klf.kir.value.f64_to_i64",
            "klf.kir.value.neg_f64",
            "klf.kir.value.abs_f64",
            "klf.kir.value.sqrt_f64",
            "klf.kir.value.floor_f64",
            "klf.kir.value.ceil_f64",
            "klf.kir.value.trunc_f64",
            "klf.kir.value.round_f64",
        ],
    ),
    ("fv->-", &["klf.kir.function.return"]),
    (
        "miii->f",
        &[
            "klf.kir.function.new_void_i64",
            "klf.kir.function.new_void_i32",
            "klf.kir.function.new_void_array_i64",
            "klf.kir.function.new_void_array_i32",
            "klf.kir.function.new_i64",
            "klf.kir.function.new_i32",
            "klf.kir.function.new_f64",
            "klf.kir.function.new_pair_i64",
            "klf.kir.function.new_pair_i32",
            "klf.kir.function.new_array_i64",
            "klf.kir.function.new_array_i32",
        ],
    ),
    (
        "mtii->f",
        &[
            "klf.kir.function.new_struct_i64",
            "klf.kir.function.new_enum_i64",
        ],
    ),
    (
        "f->b",
        &[
            "klf.kir.function.block_new",
            "klf.kir.function.block_new_i64",
            "klf.kir.function.block_new_i32",
        ],
    ),
    (
        "fbi->v",
        &[
            "klf.kir.function.block_param_i64",
            "klf.kir.function.block_param_i32",
        ],
    ),
    (
        "fb->-",
        &["klf.kir.function.switch_block", "klf.kir.function.branch"],
    ),
    (
        "fbv->-",
        &["klf.kir.function.branch_i64", "klf.kir.function.branch_i32"],
    ),
    (
        "ffv->v",
        &[
            "klf.kir.function.call_i64",
            "klf.kir.function.call_i32",
            "klf.kir.function.call_f64",
            "klf.kir.function.call_pair_i64",
            "klf.kir.function.call_pair_i32",
            "klf.kir.function.call_struct_i64",
            "klf.kir.function.call_enum_i64",
            "klf.kir.function.call_array_i64",
            "klf.kir.function.call_array_i32",
        ],
    ),
    ("ff->-", &["klf.kir.function.call_void"]),
    (
        "ffv->-",
        &[
            "klf.kir.function.call_void_i64",
            "klf.kir.function.call_void_i32",
            "klf.kir.function.call_void_array_i64",
            "klf.kir.function.call_void_array_i32",
        ],
    ),
    (
        "ffvv->-",
        &[
            "klf.kir.function.call_void2_i64",
            "klf.kir.function.call_void2_i32",
        ],
    ),
    (
        "ffvv->v",
        &["klf.kir.function.call2_i64", "klf.kir.function.call2_i32"],
    ),
    (
        "fvibb->-",
        &[
            "klf.kir.function.switch_i64",
            "klf.kir.function.switch_i32",
            "klf.kir.function.switch_enum",
        ],
    ),
    ("fvibibb->-", &["klf.kir.function.switch_enum2"]),
    (
        "f->a",
        &[
            "klf.kir.value.stack_alloc_i64",
            "klf.kir.value.stack_alloc_i32",
        ],
    ),
    (
        "fav->-",
        &[
            "klf.kir.value.store_i64",
            "klf.kir.value.assign_i64",
            "klf.kir.value.store_i32",
            "klf.kir.value.assign_i32",
        ],
    ),
    (
        "fa->-",
        &[
            "klf.kir.value.stack_dealloc_i64",
            "klf.kir.value.stack_dealloc_i32",
        ],
    ),
    (
        "fa->v",
        &["klf.kir.value.load_i64", "klf.kir.value.load_i32"],
    ),
    ("fvbb->-", &["klf.kir.function.conditional_branch"]),
    (
        "fvbvbv->-",
        &[
            "klf.kir.function.conditional_branch_i64",
            "klf.kir.function.conditional_branch_i32",
        ],
    ),
    ("fii->v", &["klf.kir.value.call_service_i64"]),
    ("fiiv->v", &["klf.kir.value.call_service_i64_1"]),
    ("fiiv->-", &["klf.kir.function.call_service_void_i64_1"]),
];

fn wrapper_spec(name: &str) -> Option<&'static str> {
    WRAPPER_SPECS
        .iter()
        .chain(shader::SHADER_WRAPPER_SPECS)
        .find(|(_, names)| names.contains(&name))
        .map(|(spec, _)| *spec)
}

pub(crate) fn spec_type(letter: u8) -> FeType {
    match letter {
        b'i' => signed_i64(),
        b'8' => FeType::Int {
            bits: 8,
            signed: false,
        },
        b'-' => FeType::Void,
        b's' => fe_handle(DefinitionHandle::Session),
        b'F' => fe_handle(DefinitionHandle::SourceFile),
        b'u' => fe_handle(DefinitionHandle::Unit),
        b'p' => fe_handle(DefinitionHandle::Program),
        b'm' => fe_handle(DefinitionHandle::KirModuleBuilder),
        b'M' => fe_handle(DefinitionHandle::KirModule),
        b't' => fe_handle(DefinitionHandle::KirTypeBuilder),
        b'T' => fe_handle(DefinitionHandle::KirType),
        b'S' => fe_handle(DefinitionHandle::KirSignature),
        b'g' => fe_handle(DefinitionHandle::KirGlobal),
        b'f' => fe_handle(DefinitionHandle::KirFunctionBuilder),
        b'b' => fe_handle(DefinitionHandle::KirBlockBuilder),
        b'v' => fe_handle(DefinitionHandle::KirValue),
        b'a' => fe_handle(DefinitionHandle::KirAddress),
        b'q' => FeType::Pointer,
        b'x' => FeType::Struct(Name::new("String")),
        b'k' => fe_handle(DefinitionHandle::ShaderModule),
        b'o' => fe_handle(DefinitionHandle::ShaderUnit),
        b'U' => FeType::Int {
            bits: 64,
            signed: false,
        },
        other => unreachable!("signature spec letter {:?}", other as char),
    }
}

/// Splits a spec into its parameter types and result type.
pub(crate) fn parse_spec(spec: &str) -> (Vec<FeType>, FeType) {
    let (params, result) = spec
        .split_once("->")
        .expect("a signature spec always has a result");
    let params = params.bytes().map(spec_type).collect();
    (params, spec_type(result.as_bytes()[0]))
}

fn wrapper_types(service: &KernelService) -> Option<(Vec<FeType>, FeType)> {
    wrapper_types_of(service.name.as_str())
}

fn wrapper_types_of(service_name: &str) -> Option<(Vec<FeType>, FeType)> {
    let Some(spec) = wrapper_spec(service_name) else {
        return definition::wrapper_types(service_name);
    };
    Some(parse_spec(spec))
}
pub(crate) fn fe_handle(handle: DefinitionHandle) -> FeType {
    FeType::DefinitionHandle(handle)
}
pub(crate) fn signed_i64() -> FeType {
    FeType::Int {
        bits: 64,
        signed: true,
    }
}
fn kir_type(ty: ServiceType) -> Type {
    match ty {
        ServiceType::Integer { bits, .. } => Type::Integer(bits),
        ServiceType::Float { bits } => Type::Float(bits),
        ServiceType::Pointer => Type::Pointer,
        ServiceType::Void => Type::Unit,
    }
}
fn foreign(name: &str, parameters: Vec<(String, Type)>, result: Type) -> Function {
    let span = Span::empty(FileId::from_u32(0), 0);
    Function {
        name: Name::new(name),
        linkage: Linkage::Private,
        abi: klf_kir::FunctionAbi::Platform,
        nounwind: false,
        foreign: true,
        c_prototype: None,
        generics: Vec::new(),
        attributes: Vec::new(),
        parameters: parameters
            .into_iter()
            .map(|(parameter_name, ty)| Parameter {
                name: Name::new(parameter_name),
                convention: None,
                context: false,
                ty,
                span,
            })
            .collect(),
        result,
        failure: None,
        engine: Engine::Native,
        blocks: Vec::new(),
        span,
    }
}
#[cfg(test)]
#[path = "builtins_tests.rs"]
mod tests;
