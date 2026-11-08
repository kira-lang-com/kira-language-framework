#![forbid(unsafe_code)]

mod analysis;
mod assembly;
mod binary;
mod bodies;
mod code;
mod constants;
mod data;
mod debug_section;
mod error;
mod function_index;
mod functions;
mod gpu_entries;
mod imports;
mod sections;
mod signature;
mod text;
mod text_entries;
mod text_entry_disassembly;
mod text_entry_encoding;
mod text_entry_specialization_validation;
mod text_entry_validation;
mod text_entry_value;
mod types;
mod unwind_section;
mod variant_index;

pub use analysis::{
    AnalysisSummary, AssemblyRegionSummary, FunctionAnalysisRecord, IndirectCallType,
    SandboxRegionSummary,
};
pub use assembly::AssemblyRecord;
pub use binary::{FormatError, Header, Section, TVISA_HEADER_SIZE, TvisaImage, TvisaImageBuilder};
pub use bodies::FunctionBodyHeader;
pub use code::{BinaryBlock, BinaryFunctionBody, BinaryInstruction, BinaryOperand};
pub use constants::ConstantRecord;
pub use data::{
    DataKind, DataLinkage, DataObject, DataRelocation, DataRelocationKind, DataSection,
};
pub use functions::{FrameSlotSummary, FunctionRecordSummary, FunctionVariant};
pub use gpu_entries::{
    GpuEntryKind, GpuEntryRecord, GpuNodeOutput, GpuNodeRecord, GpuRootRecord,
    GpuSpecializationConstant, GpuStageField, encode_gpu_entry_records,
};
pub use imports::ImportCounts;
pub use signature::{
    CAggregateDescriptor, CAggregateMember, ParameterItem, ResultItem, SignatureGroup,
    SignatureMember, TextCallingConvention, TextSignature,
};
pub use text::{
    ImageKind, TextImage, TextImageHeader, TextItem, TextItemKind, TextParseError, TvisaVersion,
    parse_text_image,
};
pub use text_entries::{TextEntryField, TextEntryValue, TextEntryValueKind, TextGpuEntry};
pub use text_entry_disassembly::{TextGpuEntryFormatter, format_gpu_entry_records};
pub use text_entry_encoding::{TextGpuEntryResolver, encode_text_gpu_entry_records};
pub use tir_format::leb;
pub use tir_format::text::{TextError, Token, TokenKind, lex};
pub use types::{
    CAggregateMemberSummary, CAggregateSummary, GroupMemberSummary, GroupSummary,
    ParameterItemSummary, ResultItemSummary, SignatureSummary, TvisaValueTypeRecord, TypeSection,
};
pub use unwind_section::{UnwindFunctionRecord, UnwindSection};
pub use variant_index::{VariantIndex, VariantSetMember, VariantSetRecord};
