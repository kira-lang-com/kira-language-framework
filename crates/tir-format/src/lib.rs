#![forbid(unsafe_code)]

mod binary;
mod binary_analysis;
mod binary_assembly;
mod binary_builder;
mod binary_capabilities;
mod binary_code;
mod binary_code_blocks;
mod binary_code_encode;
mod binary_code_instructions;
mod binary_constants;
mod binary_debug;
mod binary_generics;
mod binary_globals;
mod binary_gpu;
mod binary_imports;
mod binary_metadata;
mod binary_module;
mod binary_ridl;
#[cfg(test)]
mod binary_section_encoding_conformance;
mod binary_signatures;
mod binary_types;
mod binary_variants;
mod body;
pub mod leb;
mod module;
pub mod numeric_literals;
mod signature;
mod strings;
pub mod text;
mod text_access_assembly;
mod text_address_projection;
#[cfg(test)]
mod text_address_projection_conformance;
mod text_aggregate_constant;
#[cfg(test)]
mod text_aggregate_constant_conformance;
mod text_aggregate_constant_intern;
mod text_aggregate_constant_nested;
mod text_aggregate_destructure;
#[cfg(test)]
mod text_aggregate_destructure_conformance;
mod text_analysis_derivation;
#[cfg(test)]
mod text_array_conformance;
mod text_array_simple;
mod text_assembly;
mod text_assembly_helpers;
#[cfg(test)]
mod text_atomic_conformance;
mod text_atomic_simple;
mod text_atomic_update;
mod text_atomic_wait_notify;
mod text_block_address;
#[cfg(test)]
mod text_bytes_constant_conformance;
mod text_call_attributes;
mod text_code_strings;
mod text_code_type_roots;
mod text_gpu_builtin;
#[cfg(test)]
mod text_gpu_builtin_conformance;
mod text_gpu_fragment;
#[cfg(test)]
mod text_gpu_fragment_conformance;
mod text_gpu_mesh;
#[cfg(test)]
mod text_gpu_mesh_conformance;
mod text_gpu_printf;
#[cfg(test)]
mod text_gpu_printf_conformance;
mod text_gpu_quad;
#[cfg(test)]
mod text_gpu_quad_conformance;
mod text_gpu_spec_const;
#[cfg(test)]
mod text_gpu_spec_const_conformance;
mod text_gpu_spec_const_intern;
mod text_gpu_subgroup;
#[cfg(test)]
mod text_gpu_subgroup_conformance;
mod text_gpu_subgroup_format;
mod text_gpu_sync;
#[cfg(test)]
mod text_gpu_sync_conformance;
mod text_gpu_texture_atomic;
#[cfg(test)]
mod text_gpu_texture_atomic_conformance;
mod text_gpu_texture_basic;
#[cfg(test)]
mod text_gpu_texture_basic_conformance;
mod text_gpu_texture_sample;
#[cfg(test)]
mod text_gpu_texture_sample_conformance;
mod text_gpu_texture_sample_format;
#[cfg(test)]
mod text_gpu_texture_sparse_conformance;
mod text_gpu_tile_node;
#[cfg(test)]
mod text_gpu_tile_node_conformance;
mod text_gpu_tile_node_format;
pub use text_gpu_builtin::type_for_name as gpu_builtin_type;
mod text_const_bytes_disassembly;
mod text_const_string_disassembly;
mod text_constant_type_name;
mod text_control_flow_assembly;
#[cfg(test)]
mod text_control_flow_conformance;
mod text_control_flow_disassembly;
mod text_control_flow_dominators;
mod text_control_flow_encoding;
mod text_conversion_assembly;
#[cfg(test)]
mod text_conversion_conformance;
mod text_conversion_disassembly;
mod text_descriptor_reference;
#[cfg(test)]
mod text_descriptor_reference_conformance;
mod text_device_io;
#[cfg(test)]
mod text_device_io_conformance;
mod text_direct_call;
#[cfg(test)]
mod text_direct_call_conformance;
mod text_enum_address;
#[cfg(test)]
mod text_enum_address_conformance;
mod text_enum_payload;
#[cfg(test)]
mod text_enum_payload_conformance;
mod text_float_assembly;
#[cfg(test)]
mod text_float_constant_conformance;
mod text_fp_misc;
#[cfg(test)]
mod text_fp_misc_conformance;
#[cfg(test)]
mod text_freeze_conformance;
mod text_function_body_assembly;
mod text_function_body_disassembly;
mod text_function_body_encoding;
mod text_function_body_helpers;
mod text_function_constant_assembly;
mod text_function_reference;
#[cfg(test)]
mod text_function_reference_conformance;
mod text_global_assembly;
mod text_global_disassembly;
mod text_global_reference;
#[cfg(test)]
mod text_global_reference_conformance;
mod text_import_assembly;
mod text_indirect_branch;
#[cfg(test)]
mod text_indirect_branch_conformance;
mod text_indirect_call;
#[cfg(test)]
mod text_indirect_call_conformance;
#[cfg(test)]
mod text_integer_extended_conformance;
mod text_integer_operation_assembly;
mod text_longjmp;
mod text_managed_heap;
#[cfg(test)]
mod text_managed_heap_conformance;
mod text_memory_assembly;
mod text_memory_disassembly;
mod text_nominal_construction;
#[cfg(test)]
mod text_nominal_construction_conformance;
mod text_nominal_disassembly;
mod text_ownership_assembly;
#[cfg(test)]
mod text_ownership_conformance;
mod text_pointer_address;
#[cfg(test)]
mod text_pointer_address_conformance;
#[cfg(test)]
mod text_pointer_constant_conformance;
mod text_raw_access;
#[cfg(test)]
mod text_raw_access_conformance;
mod text_raw_heap;
#[cfg(test)]
mod text_raw_heap_conformance;
mod text_raw_memory;
#[cfg(test)]
mod text_raw_memory_conformance;
mod text_sampler_constant;
#[cfg(test)]
mod text_sampler_constant_conformance;
mod text_select_assembly;
#[cfg(test)]
mod text_semantics_flag_conformance;
mod text_signature_assembly;
mod text_signature_disassembly;
#[cfg(test)]
mod text_simple_conformance;
mod text_simple_instructions;
mod text_straight_line_assembly;
#[cfg(test)]
mod text_straight_line_conformance;
mod text_straight_line_constants;
mod text_straight_line_disassembly;
mod text_straight_line_encoding_helpers;
mod text_straight_line_operand_helpers;
mod text_straight_line_validation;
#[cfg(test)]
mod text_string_constant_conformance;
mod text_switch_enum;
#[cfg(test)]
mod text_switch_enum_conformance;
mod text_switch_int;
#[cfg(test)]
mod text_switch_int_conformance;
#[cfg(test)]
mod text_terminal_conformance;
#[cfg(test)]
mod text_tuple_conformance;
mod text_tuple_simple;
mod text_type_assembly;
mod text_type_disassembly;
#[cfg(test)]
mod text_varargs_conformance;
mod text_varargs_simple;
#[cfg(test)]
mod text_vector_conformance;
mod text_vector_constant;
#[cfg(test)]
mod text_vector_constant_conformance;
mod text_vector_constant_large;
mod text_vector_gather;
#[cfg(test)]
mod text_vector_gather_conformance;
mod text_vector_memory;
#[cfg(test)]
mod text_vector_memory_conformance;
mod text_vector_permute;
#[cfg(test)]
mod text_vector_permute_conformance;
mod text_vector_simple;
#[cfg(test)]
mod text_vector_simple_conformance;
mod type_parser;
mod types;

pub use binary::{BinaryModule, FormatError, Header, Section, TIRB_HEADER_SIZE};
pub use binary_analysis::{
    BinaryAnalysisSummary, BinaryAssemblyRegion, BinaryFunctionAnalysis, BinarySandboxRegion,
};
pub use binary_assembly::{BinaryAssemblyRecord, encode_assembly_table};
pub use binary_builder::{BinaryAssemblySourceRecord, BinaryModuleBuilder};
pub use binary_capabilities::{BinaryCapabilities, BinaryImportReference};
pub use binary_debug::{
    BinaryDebugTable, DebugCompileUnit, DebugDataRecord, DebugEnumerator, DebugFileRecord,
    DebugLabel, DebugLocation, DebugScope, DebugScopeKind, DebugSourceLocation, DebugSourceType,
    DebugTypeMember, DebugVariable,
};
pub use binary_generics::{
    BinaryAssociatedTypeBinding, BinaryAssociatedTypeRequirement, BinaryConformance,
    BinaryGenericTable, BinaryTraitDefinition, BinaryTraitMethod, BinaryVTable, BinaryVTableSlot,
    BinaryWitness,
};
pub use binary_imports::{
    BinaryImport, BinaryImportEntityKind, BinaryImportKind, BinaryImportedEntity, encode_imports,
};
pub use binary_metadata::{
    BinaryAliasDomain, BinaryAliasScope, BinaryMetadata, BinaryMetadataAttachment,
    BinaryMetadataTable,
};
pub use binary_module::{BinaryInitializer, BinaryModuleRecord, BinaryProducer};
pub use binary_ridl::{
    BinaryAbiStamp, BinaryInlinableBody, BinaryRidlExport, BinaryRidlExportKind, BinaryRidlTable,
};
pub use binary_variants::{encode_variant_sets, intern_variant_strings};
pub use body::{
    BasicBlock, BodyParseError, FunctionBody, TextFunctionSignature, TextInstruction,
    TextParameter, parse_function_body, parse_tvisa_function_body,
};
pub use module::{
    Initializer, ModuleHeader, ProducerRecord, SemanticsMode, TextItem, TextItemKind, TextModule,
    TextParseError, TirVersion, parse_text_module,
};
pub use signature::{function_has_header_attribute, function_uses_c_abi, parse_function_signature};
pub use strings::{StringTable, StringTableBuilder, StringTableError, StringTableValidator};
pub use text_assembly::{assemble_text_module, disassemble_text_module};
pub use type_parser::{TypeParseError, parse_tvisa_type, parse_type};
pub use types::{
    BinaryAddressSpace, BinaryAnnotation, BinaryBlock, BinaryBlockParameter, BinaryCodeTable,
    BinaryConstant, BinaryConstantTable, BinaryConstantValue, BinaryCopyPolicy,
    BinaryDestroyPolicy, BinaryEngine, BinaryEntryKind, BinaryField, BinaryFpMode,
    BinaryFunctionBody, BinaryFunctionParameter, BinaryFunctionType, BinaryGenericParameter,
    BinaryGenericStrategy, BinaryGlobal, BinaryGlobalTable, BinaryGpuEntry, BinaryGpuInterface,
    BinaryGpuInterfaceRole, BinaryGpuSpecializationConstant, BinaryGpuTable, BinaryInstruction,
    BinaryLayout, BinaryLinkage, BinaryNominalDeclaration, BinaryNominalDefinition,
    BinaryNominalKind, BinaryOperand, BinaryPolicy, BinaryRelocatePolicy, BinarySignature,
    BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable, BinaryVariant,
    BinaryVariantAttribute, BinaryVariantMember, BinaryVariantSet,
};
