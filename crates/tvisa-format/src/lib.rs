#![forbid(unsafe_code)]

mod analysis;
mod assembly;
mod binary;
mod binary_metadata;
mod bodies;
mod code;
mod constants;
mod data;
mod debug_section;
mod function_index;
mod functions;
mod gpu_entries;
mod image_section;
mod imports;
mod runtime;
mod sections;
mod signature;
mod text;
mod types;
mod unwind_section;
mod variant_index;

pub use analysis::{
    AnalysisSummary, AssemblyRegionSummary, FunctionAnalysisRecord, IndirectCallType,
    SandboxRegionSummary,
};
pub use assembly::{AssemblyRecord, encode_assembly_records};
pub use binary::{FormatError, Header, Section, TVISA_HEADER_SIZE, TvisaImage, TvisaImageBuilder};
pub use binary_metadata::encode_metadata_table;
pub use bodies::FunctionBodyHeader;
pub use code::{
    BinaryBlock, BinaryFunctionBody, BinaryInstruction, BinaryOperand, encode_function_bodies,
};
pub use constants::{ConstantRecord, encode_constant_records};
pub use data::{
    DataKind, DataLinkage, DataObject, DataRelocation, DataRelocationKind, DataSection,
    encode_text_data_section,
};
pub use function_index::{FunctionIndexRecord, encode_function_index};
pub use functions::{
    FrameSlotSummary, FunctionRecordSummary, FunctionVariant, encode_function_records,
    is_valid_type_tag,
};
pub use gpu_entries::{
    GPU_CAPABILITY_NAMES, GpuEntryKind, GpuEntryRecord, GpuNodeOutput, GpuNodeOutputKind,
    GpuNodeRecord, GpuRootRecord, GpuSpecializationConstant, GpuStageField,
    encode_gpu_entry_records, encode_text_gpu_entry_records,
};
pub use image_section::{ImageFunctionRecord, ImageRecord};
pub use imports::{
    ImportCounts, ImportModuleKind, ImportPayload, ImportRecord, encode_import_records,
    encode_text_import_records,
};
pub use runtime::{encode_runtime_entry_ids, encode_text_runtime_metadata};
pub use signature::{
    CAggregateDescriptor, CAggregateMember, ParameterItem, ResultItem, SignatureGroup,
    SignatureMember, TextCallingConvention, TextSignature,
};
pub use text::{
    ImageKind, TextData, TextDataElement, TextDataInitializer, TextDataRelocation,
    TextFunctionBody, TextGpuEntry, TextGpuEntryError, TextGpuEntryField, TextGpuEntryValue,
    TextGpuEntryValueKind, TextImage, TextImageFunction, TextImageHeader, TextImageMetadata,
    TextImport, TextImportModule, TextImportPayload, TextItem, TextItemKind, TextParseError,
    TextRuntimeMetadata, TextVariantArchitecture, TextVariantKey, TextVariantMember,
    TextVariantSet, TvisaVersion, parse_text_image,
};
pub use tir_format::leb;
pub use tir_format::text::{TextError, Token, TokenKind, lex};
pub use types::{
    CAggregateMemberSummary, CAggregateSummary, GroupMemberSummary, GroupSummary,
    ParameterItemSummary, ResultItemSummary, SignatureSummary, TvisaValueTypeRecord, TypeSection,
    encode_text_signature_records, encode_text_type_section,
};
pub use unwind_section::{UnwindFunctionRecord, UnwindSection};
pub use variant_index::{VariantIndex, VariantSetMember, VariantSetRecord};
