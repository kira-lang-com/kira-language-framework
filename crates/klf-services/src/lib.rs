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
mod image;
mod location;
mod observe;
mod query;
mod service;
mod session;
mod snippet;
mod shader_family;
mod solver;
mod types;

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
pub use image::{
    DefinitionHandleError, DefinitionImage, DefinitionImageError, DefinitionPhase,
    DefinitionPhaseArguments, DefinitionPhaseExecutor, DefinitionPhaseResult,
    DefinitionResultError, ExecutableDefinition,
};
pub use location::diagnostic_location;
pub use observe::{Observation, accessed, note_access, record_accesses};
pub use query::{QueryDependency, QueryId, QueryPhase, QueryRecord};
pub use service::{
    KERNEL_SERVICE_MANIFEST, KernelService, KernelServiceManifest, ServiceManifestError,
    ServiceSignature, ServiceType,
};
pub use session::{
    ArtifactId, Entity, EntityId, Expansion, LanguageDefaults, Namespace, Position, ScopeId,
    Session, SessionHandle, UnitId, overlay_key, read_through,
};
pub use snippet::{Excerpt, Rendered, render as render_snippet, runtime_diagnostic, session_diagnostic, split_message};
pub use shader_family::{SHADER_KEYS, ShaderFamily, ShaderSettings};
pub use solver::{
    Binding, Bindings, Choice, Conflict, Conformance, Requirement, Solution, Solver, TraitId,
    TraitInfo,
};
pub use types::{
    CopyPolicy, DestroyPolicy, FunctionSignature, LayoutRule, Pairing, Policy, RelocatePolicy,
    TypeId, TypeInfo, TypeKind, TypeLayout, TypeLayoutInfo,
};
