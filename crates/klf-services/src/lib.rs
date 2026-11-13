//! Kernel services (section 8): the `Session` substrate and the KIR builder
//! that the KLF Kira frontend and language definitions run against.
// Android's thread locals are emulated, and the lint misreads the
// `thread_local!` expansion there as a non-`const` initializer.
#![cfg_attr(target_os = "android", allow(clippy::missing_const_for_thread_local))]

mod arena;
mod builder;
mod builder_machine;
mod definition;
mod diagnostics;
mod driver_request;
mod file_mode;
mod filesystem_index;
mod image;
mod location;
mod observe;
mod query;
mod scheduler;
mod service;
mod session;
mod shader_family;
mod snippet;
mod solver;
mod types;
mod workspace;

pub use arena::{Arena, Id};
pub use builder::{FunctionBuilder, ModuleBuilder};
pub use definition::{
    DefaultConvention, Definition, DefinitionArtifact, DefinitionArtifactKind, DefinitionPackage,
    DefinitionRegistry, DefinitionRegistryError, DefinitionRuntimeArtifact, DefinitionSource,
    DefinitionUnit, DefinitionUnitList, DriverFlag, DriverFlagValue, EntryConvention, Exclusivity,
    FileMode, LanguageDescriptor, MemoryModel, effective_file_defaults,
};
pub use diagnostics::{DiagnosticSpec, ResolvedRuntimeDiagnostic, severity_word};
pub use driver_request::{DriverRequest, select_driver_arguments};
pub use file_mode::{FileModeSelectionError, select_file_mode};
pub use filesystem_index::{DirectoryIndex, DirectorySnapshot, shared_directory_index};
pub use image::{
    DefinitionHandleError, DefinitionImage, DefinitionImageError, DefinitionPhase,
    DefinitionPhaseArguments, DefinitionPhaseExecutor, DefinitionPhaseResult,
    DefinitionResultError, ExecutableDefinition,
};
pub use location::diagnostic_location;
pub use observe::{
    FileIdentity, Observation, accessed, directory_overlay_entries, file_identity,
    file_identity_from_metadata, note_access, note_access_many, record_accesses,
};
pub use query::{QueryDependency, QueryId, QueryPhase, QueryRecord, QueryStats};
pub use scheduler::parallel_map;
pub use service::{
    KERNEL_SERVICE_MANIFEST, KernelService, KernelServiceManifest, ServiceManifestError,
    ServiceSignature, ServiceType,
};
pub use session::{
    ArtifactId, Entity, EntityId, Expansion, LanguageDefaults, Namespace, Position, ScopeId,
    Session, SessionHandle, UnitId, overlay_key, read_through,
};
pub use shader_family::{SHADER_KEYS, ShaderFamily, ShaderSettings};
pub use snippet::{
    Excerpt, Rendered, render as render_snippet, runtime_diagnostic, session_diagnostic,
    split_message,
};
pub use solver::{
    Binding, Bindings, Choice, Conflict, Conformance, Requirement, Solution, Solver, TraitId,
    TraitInfo,
};
pub use types::{
    CopyPolicy, DestroyPolicy, FunctionSignature, LayoutRule, Pairing, Policy, RelocatePolicy,
    TypeId, TypeInfo, TypeKind, TypeLayout, TypeLayoutInfo,
};
pub use workspace::{DefinitionGraph, GraphError, PackageId, PackageNode};
