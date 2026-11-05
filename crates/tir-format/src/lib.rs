#![forbid(unsafe_code)]

mod binary;
mod binary_analysis;
mod binary_assembly;
mod binary_builder;
mod binary_code;
mod binary_code_blocks;
mod binary_code_encode;
mod binary_code_instructions;
mod binary_constants;
mod binary_generics;
mod binary_globals;
mod binary_gpu;
mod binary_imports;
mod binary_module;
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
mod text_assembly;
mod text_const_bytes_disassembly;
mod text_const_string_disassembly;
mod text_control_flow_assembly;
mod text_control_flow_disassembly;
mod text_control_flow_dominators;
mod text_control_flow_encoding;
mod text_function_body_assembly;
mod text_function_body_disassembly;
mod text_function_body_encoding;
mod text_function_body_helpers;
mod text_function_constant_assembly;
mod text_global_assembly;
mod text_global_disassembly;
mod text_import_assembly;
mod text_integer_operation_assembly;
mod text_memory_assembly;
mod text_memory_disassembly;
mod text_ownership_assembly;
mod text_select_assembly;
mod text_signature_assembly;
mod text_signature_disassembly;
mod text_straight_line_assembly;
mod text_straight_line_disassembly;
mod text_type_assembly;
mod text_type_disassembly;
mod type_parser;
mod types;

pub use binary::{BinaryModule, FormatError, Header, Section, TIRB_HEADER_SIZE};
pub use binary_analysis::{
    BinaryAnalysisSummary, BinaryAssemblyRegion, BinaryFunctionAnalysis, BinarySandboxRegion,
};
pub use binary_assembly::{BinaryAssemblyRecord, encode_assembly_table};
pub use binary_builder::{BinaryAssemblySourceRecord, BinaryModuleBuilder};
pub use binary_generics::{
    BinaryAssociatedTypeBinding, BinaryAssociatedTypeRequirement, BinaryConformance,
    BinaryGenericTable, BinaryTraitDefinition, BinaryTraitMethod, BinaryVTable, BinaryVTableSlot,
    BinaryWitness,
};
pub use binary_imports::{
    BinaryImport, BinaryImportEntityKind, BinaryImportKind, BinaryImportedEntity, encode_imports,
};
pub use binary_module::{BinaryInitializer, BinaryModuleRecord, BinaryProducer};
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
