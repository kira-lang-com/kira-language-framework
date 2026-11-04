#![forbid(unsafe_code)]

mod binary;
mod binary_analysis;
mod binary_assembly;
mod binary_capabilities;
mod binary_code;
mod binary_code_blocks;
mod binary_code_instructions;
mod binary_constants;
mod binary_debug;
mod binary_generics;
mod binary_globals;
mod binary_gpu;
mod binary_gpu_node;
mod binary_imports;
mod binary_metadata;
mod binary_module;
mod binary_ridl;
mod binary_signatures;
mod binary_types;
mod binary_variants;
mod body;
mod gpu_sync;
pub mod leb;
mod module;
pub mod numeric_literals;
mod signature;
mod strings;
pub mod text;
mod text_binary;
mod type_parser;
mod types;

pub use binary::{
    BinaryModule, FormatError, Header, Section, TIRB_FLAG_CANONICAL, TIRB_HEADER_SIZE,
};
pub use binary_analysis::{
    BinaryAnalysisSummary, BinaryAssemblyRegion, BinaryFunctionAnalysis, BinarySandboxRegion,
};
pub use binary_assembly::{BinaryAssemblyRecord, encode_assembly_table};
pub use binary_capabilities::{BinaryCapabilities, BinaryImportReference};
pub use binary_debug::{
    BinaryDebugTable, DebugCompileUnit, DebugDataRecord, DebugEnumerator, DebugFileRecord,
    DebugLabel, DebugLocation, DebugScope, DebugScopeKind, DebugSourceLocation, DebugSourceType,
    DebugTypeMember, DebugVariable, encode_binary_debug_table,
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
    BinaryMetadataTable, MetadataDecodeError, MetadataEncodeError,
};
pub use binary_module::{BinaryInitializer, BinaryModuleRecord, BinaryProducer};
pub use binary_ridl::{
    BinaryAbiStamp, BinaryInlinableBody, BinaryRidlExport, BinaryRidlExportKind, BinaryRidlTable,
};
pub use binary_variants::encode_variant_sets;
pub use body::{
    BasicBlock, BodyParseError, FunctionBody, TextDebugAttachment, TextFunctionSignature,
    TextInstruction, TextParameter, parse_function_body, parse_tvisa_function_body,
};
pub use gpu_sync::{
    GpuMemoryDomain, GpuMemoryDomains, GpuSynchronization, GpuSynchronizationBinaryError,
    GpuSynchronizationKind, GpuSynchronizationParseError, GpuSynchronizationScope,
    parse_binary_gpu_synchronization, parse_gpu_synchronization,
};
pub use module::{
    Initializer, ModuleHeader, ProducerRecord, SemanticsMode, TextItem, TextItemKind, TextModule,
    TextParseError, TirVersion, parse_text_module,
};
pub use signature::{function_uses_c_abi, parse_function_signature};
pub use strings::{StringTable, StringTableError, StringTableValidator};
pub use text_binary::{assemble_text_module, disassemble_binary_module};
pub use type_parser::{TypeParseError, parse_tvisa_type, parse_type};
pub use types::{
    BinaryAddressSpace, BinaryAnnotation, BinaryBlock, BinaryBlockParameter, BinaryCodeTable,
    BinaryConstant, BinaryConstantTable, BinaryConstantValue, BinaryCopyPolicy,
    BinaryDestroyPolicy, BinaryEngine, BinaryEntryKind, BinaryField, BinaryFpMode,
    BinaryFunctionBody, BinaryFunctionParameter, BinaryFunctionType, BinaryGenericParameter,
    BinaryGenericStrategy, BinaryGlobal, BinaryGlobalTable, BinaryGpuEntry, BinaryGpuInterface,
    BinaryGpuInterfaceRole, BinaryGpuNodeOutput, BinaryGpuNodeOutputKind, BinaryGpuNodeRecord,
    BinaryGpuSpecializationConstant, BinaryGpuTable, BinaryInstruction, BinaryLayout,
    BinaryLinkage, BinaryNominalDeclaration, BinaryNominalDefinition, BinaryNominalKind,
    BinaryOperand, BinaryPolicy, BinaryRelocatePolicy, BinarySignature, BinarySignatureTable,
    BinaryTarget, BinaryType, BinaryTypeTable, BinaryVariant, BinaryVariantAttribute,
    BinaryVariantMember, BinaryVariantSet,
};
