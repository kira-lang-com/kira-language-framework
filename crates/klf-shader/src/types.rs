//! KSIR types, address spaces and interface bindings (sections 8 to 10).

use crate::Span;
use crate::arena::Handle;
use crate::module::Override;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScalarKind {
    Bool,
    Sint,
    Uint,
    Float,
}

impl ScalarKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Sint => "sint",
            Self::Uint => "uint",
            Self::Float => "float",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "bool" => Some(Self::Bool),
            "sint" => Some(Self::Sint),
            "uint" => Some(Self::Uint),
            "float" => Some(Self::Float),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_numeric(self) -> bool {
        !matches!(self, Self::Bool)
    }

    #[must_use]
    pub const fn is_integer(self) -> bool {
        matches!(self, Self::Sint | Self::Uint)
    }
}

/// A scalar: its kind and its width in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scalar {
    pub kind: ScalarKind,
    pub width: u8,
}

impl Scalar {
    pub const BOOL: Self = Self {
        kind: ScalarKind::Bool,
        width: 1,
    };
    pub const I32: Self = Self {
        kind: ScalarKind::Sint,
        width: 4,
    };
    pub const U32: Self = Self {
        kind: ScalarKind::Uint,
        width: 4,
    };
    pub const F32: Self = Self {
        kind: ScalarKind::Float,
        width: 4,
    };
    pub const F16: Self = Self {
        kind: ScalarKind::Float,
        width: 2,
    };

    /// The text spelling: `bool`, `i32`, `u32`, `f32`, `f16`, ...
    #[must_use]
    pub fn text(self) -> String {
        match self.kind {
            ScalarKind::Bool => "bool".to_owned(),
            ScalarKind::Sint => format!("i{}", u32::from(self.width) * 8),
            ScalarKind::Uint => format!("u{}", u32::from(self.width) * 8),
            ScalarKind::Float => format!("f{}", u32::from(self.width) * 8),
        }
    }

    #[must_use]
    pub fn from_text(text: &str) -> Option<Self> {
        let (kind, bits) = match text {
            "bool" => return Some(Self::BOOL),
            _ if text.starts_with('i') => (ScalarKind::Sint, &text[1..]),
            _ if text.starts_with('u') => (ScalarKind::Uint, &text[1..]),
            _ if text.starts_with('f') => (ScalarKind::Float, &text[1..]),
            _ => return None,
        };
        let width = match (kind, bits) {
            (ScalarKind::Float, "16") => 2,
            (_, "32") => 4,
            (_, "64") => 8,
            _ => return None,
        };
        Some(Self { kind, width })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageDim {
    D1,
    D2,
    D3,
    Cube,
}

impl ImageDim {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::D1 => "1d",
            Self::D2 => "2d",
            Self::D3 => "3d",
            Self::Cube => "cube",
        }
    }

    /// Components of a coordinate into an image of this dimension.
    #[must_use]
    pub const fn coordinates(self) -> u8 {
        match self {
            Self::D1 => 1,
            Self::D2 => 2,
            Self::D3 | Self::Cube => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StorageFormat {
    R8Unorm,
    Rg8Unorm,
    Rgba8Unorm,
    Rgba8Snorm,
    Rgba8Uint,
    Rgba8Sint,
    Bgra8Unorm,
    R16Float,
    Rg16Float,
    Rgba16Float,
    Rgba16Uint,
    Rgba16Sint,
    R32Float,
    R32Uint,
    R32Sint,
    Rg32Float,
    Rg32Uint,
    Rg32Sint,
    Rgba32Float,
    Rgba32Uint,
    Rgba32Sint,
    Rgb10a2Unorm,
    Rg11b10Ufloat,
}

impl StorageFormat {
    pub const ALL: [Self; 23] = [
        Self::R8Unorm,
        Self::Rg8Unorm,
        Self::Rgba8Unorm,
        Self::Rgba8Snorm,
        Self::Rgba8Uint,
        Self::Rgba8Sint,
        Self::Bgra8Unorm,
        Self::R16Float,
        Self::Rg16Float,
        Self::Rgba16Float,
        Self::Rgba16Uint,
        Self::Rgba16Sint,
        Self::R32Float,
        Self::R32Uint,
        Self::R32Sint,
        Self::Rg32Float,
        Self::Rg32Uint,
        Self::Rg32Sint,
        Self::Rgba32Float,
        Self::Rgba32Uint,
        Self::Rgba32Sint,
        Self::Rgb10a2Unorm,
        Self::Rg11b10Ufloat,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::R8Unorm => "r8unorm",
            Self::Rg8Unorm => "rg8unorm",
            Self::Rgba8Unorm => "rgba8unorm",
            Self::Rgba8Snorm => "rgba8snorm",
            Self::Rgba8Uint => "rgba8uint",
            Self::Rgba8Sint => "rgba8sint",
            Self::Bgra8Unorm => "bgra8unorm",
            Self::R16Float => "r16float",
            Self::Rg16Float => "rg16float",
            Self::Rgba16Float => "rgba16float",
            Self::Rgba16Uint => "rgba16uint",
            Self::Rgba16Sint => "rgba16sint",
            Self::R32Float => "r32float",
            Self::R32Uint => "r32uint",
            Self::R32Sint => "r32sint",
            Self::Rg32Float => "rg32float",
            Self::Rg32Uint => "rg32uint",
            Self::Rg32Sint => "rg32sint",
            Self::Rgba32Float => "rgba32float",
            Self::Rgba32Uint => "rgba32uint",
            Self::Rgba32Sint => "rgba32sint",
            Self::Rgb10a2Unorm => "rgb10a2unorm",
            Self::Rg11b10Ufloat => "rg11b10ufloat",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.name() == name)
    }

    /// The scalar kind a load from the format yields.
    #[must_use]
    pub const fn kind(self) -> ScalarKind {
        match self {
            Self::Rgba8Uint
            | Self::Rgba16Uint
            | Self::R32Uint
            | Self::Rg32Uint
            | Self::Rgba32Uint => ScalarKind::Uint,
            Self::Rgba8Sint
            | Self::Rgba16Sint
            | Self::R32Sint
            | Self::Rg32Sint
            | Self::Rgba32Sint => ScalarKind::Sint,
            _ => ScalarKind::Float,
        }
    }
}

/// A subset of `{load, store, atomic}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Access {
    pub load: bool,
    pub store: bool,
    pub atomic: bool,
}

impl Access {
    pub const LOAD: Self = Self {
        load: true,
        store: false,
        atomic: false,
    };
    pub const LOAD_STORE: Self = Self {
        load: true,
        store: true,
        atomic: false,
    };
    pub const STORE: Self = Self {
        load: false,
        store: true,
        atomic: false,
    };

    #[must_use]
    pub fn words(self) -> Vec<&'static str> {
        let mut words = Vec::new();
        if self.load {
            words.push("load");
        }
        if self.store {
            words.push("store");
        }
        if self.atomic {
            words.push("atomic");
        }
        words
    }

    #[must_use]
    pub fn from_words<'a>(words: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut access = Self::default();
        for word in words {
            match word {
                "load" => access.load = true,
                "store" => access.store = true,
                "atomic" => access.atomic = true,
                _ => return None,
            }
        }
        Some(access)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        !(self.load || self.store || self.atomic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageClass {
    Sampled {
        kind: ScalarKind,
        multisampled: bool,
    },
    Depth {
        multisampled: bool,
    },
    Storage {
        format: StorageFormat,
        access: Access,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressSpace {
    Function,
    Private,
    Workgroup,
    Uniform,
    Storage { access: Access },
    Handle,
    Immediate,
}

impl AddressSpace {
    /// Whether a load through a pointer in this space is allowed.
    #[must_use]
    pub const fn loads(self) -> bool {
        match self {
            Self::Storage { access } => access.load,
            Self::Handle => false,
            _ => true,
        }
    }

    /// Whether a store through a pointer in this space is allowed.
    #[must_use]
    pub const fn stores(self) -> bool {
        match self {
            Self::Function | Self::Private | Self::Workgroup => true,
            Self::Storage { access } => access.store,
            Self::Uniform | Self::Handle | Self::Immediate => false,
        }
    }

    #[must_use]
    pub const fn is_host_shared(self) -> bool {
        matches!(self, Self::Uniform | Self::Storage { .. } | Self::Immediate)
    }

    #[must_use]
    pub const fn is_resource(self) -> bool {
        matches!(self, Self::Uniform | Self::Storage { .. } | Self::Handle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArraySize {
    Constant(u32),
    Override(Handle<Override>),
    Runtime,
}

/// The rule a struct's offsets were computed under (section 8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutRule {
    Std140,
    Std430,
    Scalar,
    Msl,
    Explicit,
    None,
}

impl LayoutRule {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Std140 => "std140",
            Self::Std430 => "std430",
            Self::Scalar => "scalar",
            Self::Msl => "msl",
            Self::Explicit => "explicit",
            Self::None => "none",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Std140,
            Self::Std430,
            Self::Scalar,
            Self::Msl,
            Self::Explicit,
            Self::None,
        ]
        .into_iter()
        .find(|rule| rule.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltIn {
    Position,
    VertexIndex,
    InstanceIndex,
    FrontFacing,
    FragDepth,
    SampleIndex,
    SampleMask,
    GlobalInvocationId,
    LocalInvocationId,
    LocalInvocationIndex,
    WorkgroupId,
    NumWorkgroups,
    ClipDistances,
}

impl BuiltIn {
    pub const ALL: [Self; 13] = [
        Self::Position,
        Self::VertexIndex,
        Self::InstanceIndex,
        Self::FrontFacing,
        Self::FragDepth,
        Self::SampleIndex,
        Self::SampleMask,
        Self::GlobalInvocationId,
        Self::LocalInvocationId,
        Self::LocalInvocationIndex,
        Self::WorkgroupId,
        Self::NumWorkgroups,
        Self::ClipDistances,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::VertexIndex => "vertex_index",
            Self::InstanceIndex => "instance_index",
            Self::FrontFacing => "front_facing",
            Self::FragDepth => "frag_depth",
            Self::SampleIndex => "sample_index",
            Self::SampleMask => "sample_mask",
            Self::GlobalInvocationId => "global_invocation_id",
            Self::LocalInvocationId => "local_invocation_id",
            Self::LocalInvocationIndex => "local_invocation_index",
            Self::WorkgroupId => "workgroup_id",
            Self::NumWorkgroups => "num_workgroups",
            Self::ClipDistances => "clip_distances",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|builtin| builtin.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Interpolation {
    Perspective,
    Linear,
    Flat,
}

impl Interpolation {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Linear => "linear",
            Self::Flat => "flat",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sampling {
    Center,
    Centroid,
    Sample,
}

impl Sampling {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Centroid => "centroid",
            Self::Sample => "sample",
        }
    }
}

/// How an interface value is bound (section 10.3). A `None` location is
/// `auto`, which normalization replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    BuiltIn(BuiltIn),
    Location {
        location: Option<u32>,
        interpolation: Option<Interpolation>,
        sampling: Option<Sampling>,
        blend_src: Option<u32>,
    },
}

impl Binding {
    #[must_use]
    pub const fn auto() -> Self {
        Self::Location {
            location: None,
            interpolation: None,
            sampling: None,
            blend_src: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructMember {
    pub name: String,
    pub ty: Handle<Type>,
    pub binding: Option<Binding>,
    pub offset: u32,
    pub size: u32,
    pub align: u32,
    /// Where the definition declared the member, for the editor
    /// (`KLF-EDITOR.md` section 14.4).
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeInner {
    Scalar(Scalar),
    Vector {
        size: u8,
        scalar: Scalar,
    },
    Matrix {
        columns: u8,
        rows: u8,
        scalar: Scalar,
    },
    Atomic(Scalar),
    Pointer {
        base: Handle<Type>,
        space: AddressSpace,
    },
    Array {
        base: Handle<Type>,
        size: ArraySize,
        stride: u32,
    },
    Struct {
        rule: LayoutRule,
        members: Vec<StructMember>,
        size: u32,
        align: u32,
    },
    Image {
        dim: ImageDim,
        arrayed: bool,
        class: ImageClass,
    },
    Sampler {
        comparison: bool,
    },
}

impl TypeInner {
    #[must_use]
    pub const fn scalar(&self) -> Option<Scalar> {
        match *self {
            Self::Scalar(scalar)
            | Self::Vector { scalar, .. }
            | Self::Matrix { scalar, .. }
            | Self::Atomic(scalar) => Some(scalar),
            _ => None,
        }
    }

    /// The scalar of a scalar or vector.
    #[must_use]
    pub const fn scalar_or_vector(&self) -> Option<(Option<u8>, Scalar)> {
        match *self {
            Self::Scalar(scalar) => Some((None, scalar)),
            Self::Vector { size, scalar } => Some((Some(size), scalar)),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_handle(&self) -> bool {
        matches!(self, Self::Image { .. } | Self::Sampler { .. })
    }
}

/// A type: interned by its name and shape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Type {
    pub name: Option<String>,
    pub inner: TypeInner,
}

/// A capability a module may declare (section 16.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    Float16,
    Float64,
    Int64,
    Int64Atomics,
    ImageAtomics,
    CubeArray,
    Multisampled,
    SampleShading,
    ClipDistances,
    DualSourceBlending,
    StorageImageReadWrite,
}

impl Capability {
    pub const ALL: [Self; 11] = [
        Self::Float16,
        Self::Float64,
        Self::Int64,
        Self::Int64Atomics,
        Self::ImageAtomics,
        Self::CubeArray,
        Self::Multisampled,
        Self::SampleShading,
        Self::ClipDistances,
        Self::DualSourceBlending,
        Self::StorageImageReadWrite,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Float16 => "float16",
            Self::Float64 => "float64",
            Self::Int64 => "int64",
            Self::Int64Atomics => "int64-atomics",
            Self::ImageAtomics => "image-atomics",
            Self::CubeArray => "cube-array",
            Self::Multisampled => "multisampled",
            Self::SampleShading => "sample-shading",
            Self::ClipDistances => "clip-distances",
            Self::DualSourceBlending => "dual-source-blending",
            Self::StorageImageReadWrite => "storage-image-read-write",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }
}
