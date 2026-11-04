mod atomics;
mod borrow_scopes;
mod borrowed_uses;
mod coroutines;
mod heap;
mod loadable;
mod owned_values;
mod profile;
mod signatures;
mod stack_allocations;
mod static_access;
mod structure;
mod types;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryCodeTable, BinaryConstantTable, BinaryFunctionBody, BinaryGenericTable,
    BinaryGlobalTable, BinaryGpuTable, BinaryImport, BinarySignatureTable, BinaryTarget,
    BinaryTypeTable, StringTable,
};
use tir_target::Profile;

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);

pub(super) fn infer_value_types(
    body: &BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
) -> Vec<Option<u64>> {
    types::infer_value_types_with_imports(body, signatures, types, imports)
}

pub(super) fn witness_or_vtable_function_type(
    instruction: &tir_format::BinaryInstruction,
    signatures: &BinarySignatureTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
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

pub(super) fn verify_generic_witness_signatures(
    generics: &BinaryGenericTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<Diagnostic> {
    signatures::verify_generic_witness_signatures(generics, signatures, types)
}

pub(super) fn verify(
    code: &BinaryCodeTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    generics: Option<&BinaryGenericTable>,
    strings: Option<&StringTable>,
    imports: Option<&[BinaryImport]>,
    globals: Option<&BinaryGlobalTable>,
    gpu: Option<&BinaryGpuTable>,
    constants: Option<&BinaryConstantTable>,
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
        let value_types = types::infer_value_types_with_imports(body, signatures, types, imports);
        if let Some(signature) = signature {
            diagnostics.extend(borrow_scopes::verify(body, &signature.name));
            diagnostics.extend(static_access::verify(body, &signature.name));
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
        let structured = signature.is_some_and(|signature| {
            signature.structured
                || matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both)
        });
        diagnostics.extend(structure::verify_body(body, &value_types, structured));
        diagnostics.extend(stack_allocations::verify(
            body,
            &signature.map_or("<unknown>", |s| s.name.as_str()),
        ));
        if let Some(signature) = signature {
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
            if matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both) {
                diagnostics.extend(types::gpu::verify_texture_instructions(
                    body,
                    &value_types,
                    types,
                    &signature.name,
                ));
                diagnostics.extend(types::gpu::verify_ray_instructions(
                    body,
                    &value_types,
                    types,
                    &signature.name,
                ));
                diagnostics.extend(types::gpu::verify_advanced_instructions(
                    body,
                    &value_types,
                    types,
                    &signature.name,
                ));
                diagnostics.extend(types::gpu::verify_geometry_instructions(
                    body,
                    &value_types,
                    types,
                    &signature.name,
                ));
                diagnostics.extend(types::gpu::verify_collective_instructions(
                    body,
                    &value_types,
                    types,
                    &signature.name,
                ));
            }
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
                imports,
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
        constants,
        module_profile,
    ));
    diagnostics.extend(tir_uniformity::verify_binary_gpu_module(
        code, signatures, gpu, generics, strings, imports,
    ));
    diagnostics
}
