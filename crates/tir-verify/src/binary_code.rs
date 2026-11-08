mod atomics;
mod borrow_scopes;
mod borrowed_uses;
mod coroutines;
pub(crate) mod gpu_builtin;
pub(crate) mod gpu_fragment;
mod gpu_mesh;
pub(crate) mod gpu_printf;
mod gpu_quad;
pub(crate) mod gpu_spec_const;
mod gpu_subgroup;
pub(crate) mod gpu_sync;
mod gpu_texture_atomic;
mod gpu_texture_basic;
mod gpu_texture_sample;
mod gpu_tile_node;
mod heap;
mod loadable;
mod owned_values;
mod profile;
mod signatures;
mod static_access;
mod structure;
mod types;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryFunctionBody, BinaryGenericTable, BinaryGlobalTable, BinaryGpuTable,
    BinaryImport, BinarySignatureTable, BinaryTypeTable, StringTable,
};
use tir_target::Profile;

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);

pub(super) fn infer_value_types(
    body: &BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<Option<u64>> {
    types::infer_value_types(body, signatures, types)
}

pub(super) fn is_type_trivial(types: &BinaryTypeTable, ty: u64) -> bool {
    types::aggregate_instructions::is_trivial_type(types, ty)
}

pub(super) fn witness_or_vtable_function_type(
    instruction: &tir_format::BinaryInstruction,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&tir_format::StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
) -> Option<u64> {
    owned_values::witness_or_vtable_function_type(
        instruction,
        signatures,
        generics,
        strings,
        imports,
        globals,
    )
}

pub(super) fn verify(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: Option<&tir_format::BinaryConstantTable>,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    gpu: Option<&BinaryGpuTable>,
    module_profile: Profile,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for body in &code.functions {
        let signature = signatures.records.get(body.function_index as usize);
        if body.blocks.iter().any(|block| block.instructions.is_none()) {
            let name = signature.map_or("<unknown>", |signature| signature.name.as_str());
            diagnostics.push(Diagnostic::error(
                V1,
                format!("function @{name} contains an unsupported or unknown CODE opcode"),
            ));
            continue;
        }
        let value_types = types::infer_value_types(body, signatures, types);
        diagnostics.extend(gpu_texture_sample::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_texture_atomic::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_texture_basic::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_tile_node::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_mesh::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_subgroup::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_quad::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_printf::verify(
            body,
            signature.map(|signature| signature.target),
            strings,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_fragment::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_spec_const::verify(
            body,
            signature.map(|signature| signature.target),
            types,
            constants,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_sync::verify(
            body,
            signature.map(|signature| signature.target),
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(gpu_builtin::verify(
            body,
            signature.map(|signature| signature.target),
            types,
            strings,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(types::verify_symbol_instructions(
            body,
            types,
            globals,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(types::verify_function_references(
            body,
            signatures,
            types,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(types::verify_closure_contexts(
            body,
            &value_types,
            types,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(types::verify_closure_callbacks(
            body,
            types,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        if let Some(signature) = signature {
            diagnostics.extend(borrow_scopes::verify(body, &signature.name));
            diagnostics.extend(static_access::verify(body, &signature.name));
            if !signature.returns_twice {
                for instruction in body
                    .blocks
                    .iter()
                    .flat_map(|block| block.instructions.iter().flatten())
                    .filter(|instruction| instruction.opcode == 0x214)
                {
                    diagnostics.push(Diagnostic::error(
                        Code::new(Dialect::Tir, Area::Verifier, 2),
                        format!(
                            "function @{} CODE offset {}: setjmp requires returns_twice",
                            signature.name, instruction.offset
                        ),
                    ));
                }
            }
        }
        diagnostics.extend(borrowed_uses::verify(
            body,
            &value_types,
            signatures,
            types,
            generics,
            strings,
            imports,
            globals,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(owned_values::verify(
            body,
            &value_types,
            signatures,
            types,
            generics,
            strings,
            imports,
            globals,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        diagnostics.extend(loadable::verify(
            body,
            &value_types,
            types,
            signature.map_or("<unknown>", |signature| signature.name.as_str()),
        ));
        if let Some(signature) = signature {
            diagnostics.extend(atomics::verify(
                body,
                &value_types,
                types,
                signature.target,
                &signature.name,
            ));
        }
        diagnostics.extend(structure::verify_body(body, &value_types));
        if let Some(signature) = signature {
            diagnostics.extend(types::verify_indirect_branch_integrity(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_control_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_varargs_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_device_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_hint_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_scalar_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_vector_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_aggregate_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(types::verify_address_instructions(
                body,
                &value_types,
                types,
                &signature.name,
                signature.target,
            ));
            diagnostics.extend(types::verify_memory_instructions(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(heap::verify_body(
                body,
                &value_types,
                types,
                &signature.name,
            ));
            diagnostics.extend(signatures::verify_body(
                body,
                signature,
                signatures,
                types,
                &value_types,
                generics,
                strings,
                imports,
                globals,
            ));
            diagnostics.extend(coroutines::verify_body(
                body,
                signature,
                signatures,
                types,
                &value_types,
            ));
        }
    }
    diagnostics.extend(profile::verify(
        code,
        signatures,
        types,
        generics,
        strings,
        imports,
        globals,
        gpu,
        module_profile,
    ));
    diagnostics.extend(tir_uniformity::verify_binary_gpu_module(
        code, signatures, gpu, strings,
    ));
    diagnostics
}
