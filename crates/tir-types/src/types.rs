use std::collections::HashMap;
use std::fmt;

use tir_core::TypeId;

use crate::TypeError;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FloatType {
    F16,
    Bf16,
    F32,
    F64,
    F128,
    F8E4M3,
    F8E5M2,
    F6E2M3,
    F6E3M2,
    F4E2M1,
    E8M0,
}

impl FloatType {
    #[must_use]
    pub const fn bits(self) -> u16 {
        match self {
            Self::F16 | Self::Bf16 => 16,
            Self::F32 => 32,
            Self::F64 => 64,
            Self::F128 => 128,
            Self::F8E4M3 | Self::F8E5M2 | Self::E8M0 => 8,
            Self::F6E2M3 | Self::F6E3M2 => 6,
            Self::F4E2M1 => 4,
        }
    }
}

impl fmt::Display for FloatType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::F16 => "f16",
            Self::Bf16 => "bf16",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::F128 => "f128",
            Self::F8E4M3 => "f8e4m3",
            Self::F8E5M2 => "f8e5m2",
            Self::F6E2M3 => "f6e2m3",
            Self::F6E3M2 => "f6e3m2",
            Self::F4E2M1 => "f4e2m1",
            Self::E8M0 => "e8m0",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AddressSpace {
    Generic,
    Device,
    Constant,
    Threadgroup,
    Thread,
    ObjectPayload,
    RayPayload,
    HitAttribute,
    Tile,
    NodeRecord,
}

impl AddressSpace {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Device => "device",
            Self::Constant => "constant",
            Self::Threadgroup => "threadgroup",
            Self::Thread => "thread",
            Self::ObjectPayload => "object_payload",
            Self::RayPayload => "ray_payload",
            Self::HitAttribute => "hit_attribute",
            Self::Tile => "tile",
            Self::NodeRecord => "node_record",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Abi {
    Ridl,
    C,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ParameterType {
    pub context: bool,
    pub convention: Option<Convention>,
    pub ty: Type,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Convention {
    Owned,
    Borrow,
    BorrowMut,
}

impl fmt::Display for Convention {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Owned => "owned",
            Self::Borrow => "borrow",
            Self::BorrowMut => "borrowmut",
        })
    }
}

impl fmt::Display for ParameterType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.context {
            formatter.write_str("context ")?;
        }
        if let Some(convention) = self.convention {
            write!(formatter, "{convention} ")?;
        }
        fmt::Display::fmt(&self.ty, formatter)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FunctionType {
    pub parameters: Vec<ParameterType>,
    pub result: Box<Type>,
    pub failure: Option<Box<Type>>,
    pub asynchronous: bool,
    pub abi: Abi,
    pub foreign: bool,
}

impl FunctionType {
    fn fmt_signature(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("(")?;
        write_joined(formatter, &self.parameters)?;
        write!(formatter, ") -> {}", self.result)?;
        if let Some(failure) = &self.failure {
            write!(formatter, " fails {failure}")?;
        }
        if self.asynchronous {
            formatter.write_str(" async")?;
        }
        if self.abi == Abi::C {
            formatter.write_str(" abi(c)")?;
        }
        if self.foreign {
            formatter.write_str(" foreign")?;
        }
        Ok(())
    }
}

impl fmt::Display for FunctionType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fn")?;
        self.fmt_signature(formatter)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClosureType {
    pub borrowed: bool,
    pub signature: FunctionType,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextureDimension {
    D1,
    D2,
    D3,
    Cube,
    D1Array,
    D2Array,
    CubeArray,
}

impl TextureDimension {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::D1 => "1d",
            Self::D2 => "2d",
            Self::D3 => "3d",
            Self::Cube => "cube",
            Self::D1Array => "1darray",
            Self::D2Array => "2darray",
            Self::CubeArray => "cubearray",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextureAccess {
    Sample,
    Read,
    Write,
    ReadWrite,
}

impl TextureAccess {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Read => "read",
            Self::Write => "write",
            Self::ReadWrite => "readwrite",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MatrixScope {
    Subgroup,
    Threadgroup,
}

impl MatrixScope {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Subgroup => "subgroup",
            Self::Threadgroup => "threadgroup",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CooperativeMatrixUse {
    A,
    B,
    Acc,
}

impl CooperativeMatrixUse {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::Acc => "acc",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntryKind {
    Compute,
    Object,
    Mesh,
    Vertex,
    Fragment,
    Tile,
    Raygen,
    Intersection,
    Anyhit,
    Closesthit,
    Miss,
    Callable,
    Node,
}

impl EntryKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Object => "object",
            Self::Mesh => "mesh",
            Self::Vertex => "vertex",
            Self::Fragment => "fragment",
            Self::Tile => "tile",
            Self::Raygen => "raygen",
            Self::Intersection => "intersection",
            Self::Anyhit => "anyhit",
            Self::Closesthit => "closesthit",
            Self::Miss => "miss",
            Self::Callable => "callable",
            Self::Node => "node",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    Integer(u16),
    Float(FloatType),
    Pointer(AddressSpace),
    Code,
    Unit,
    Never,
    FixedVector {
        lanes: u32,
        element: Box<Type>,
    },
    ScalableVector(Box<Type>),
    Tuple(Vec<Type>),
    Array {
        count: u64,
        element: Box<Type>,
    },
    Nominal {
        path: String,
        arguments: Vec<Type>,
    },
    Generic(String),
    Reference(Box<Type>),
    Unique(Box<Type>),
    Weak(Box<Type>),
    Address {
        pointee: Box<Type>,
        space: Option<AddressSpace>,
    },
    Function(FunctionType),
    Closure(ClosureType),
    Coroutine {
        yield_type: Box<Type>,
        resume_type: Box<Type>,
        result_type: Box<Type>,
    },
    Frame(String),
    Texture {
        dimension: TextureDimension,
        component: Box<Type>,
        access: TextureAccess,
        multisampled: bool,
    },
    Sampler,
    AccelerationStructure,
    VisibleFunctionTable(Box<FunctionType>),
    Tensor {
        count: u32,
        element: Box<Type>,
    },
    RayQuery,
    HitObject,
    CooperativeMatrix {
        scope: MatrixScope,
        rows: u32,
        columns: u32,
        element: Box<Type>,
        use_kind: CooperativeMatrixUse,
    },
    Entry {
        kind: EntryKind,
        root: Box<Type>,
        immediate: Box<Type>,
    },
}

impl Type {
    pub fn validate(&self) -> Result<(), TypeError> {
        crate::validation::validate_type(self)
    }

    /// Validates a type in the TVISA value type subset.
    pub fn validate_tvisa(&self) -> Result<(), TypeError> {
        crate::validation::validate_tvisa_type(self)
    }

    pub fn canonical_text(&self) -> Result<String, TypeError> {
        self.validate()?;
        Ok(self.to_string())
    }

    pub fn type_id(&self) -> Result<TypeId, TypeError> {
        if crate::canonical::contains_generic(self) {
            return Err(TypeError::UnsubstitutedGeneric);
        }
        if !crate::canonical::paths_qualified(self) {
            return Err(TypeError::UnqualifiedNominalPath);
        }
        Ok(TypeId::from_canonical_text(&self.canonical_text()?))
    }

    pub fn canonical_text_in(
        &self,
        module_path: &str,
        bindings: &HashMap<String, Self>,
    ) -> Result<String, TypeError> {
        let specialized = self.substitute(bindings);
        if crate::canonical::contains_generic(&specialized) {
            return Err(TypeError::UnsubstitutedGeneric);
        }
        let qualified = crate::canonical::qualify(&specialized, module_path);
        if !crate::canonical::paths_qualified(&qualified) {
            return Err(TypeError::UnqualifiedNominalPath);
        }
        qualified.canonical_text()
    }

    pub fn type_id_in(
        &self,
        module_path: &str,
        bindings: &HashMap<String, Self>,
    ) -> Result<TypeId, TypeError> {
        Ok(TypeId::from_canonical_text(
            &self.canonical_text_in(module_path, bindings)?,
        ))
    }

    #[must_use]
    pub fn substitute(&self, bindings: &HashMap<String, Self>) -> Self {
        match self {
            Self::Generic(name) => bindings.get(name).cloned().unwrap_or_else(|| self.clone()),
            Self::FixedVector { lanes, element } => Self::FixedVector {
                lanes: *lanes,
                element: Box::new(element.substitute(bindings)),
            },
            Self::ScalableVector(element) => {
                Self::ScalableVector(Box::new(element.substitute(bindings)))
            }
            Self::Tuple(elements) => Self::Tuple(
                elements
                    .iter()
                    .map(|element| element.substitute(bindings))
                    .collect(),
            ),
            Self::Array { count, element } => Self::Array {
                count: *count,
                element: Box::new(element.substitute(bindings)),
            },
            Self::Nominal { path, arguments } => Self::Nominal {
                path: path.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| argument.substitute(bindings))
                    .collect(),
            },
            Self::Reference(inner) => Self::Reference(Box::new(inner.substitute(bindings))),
            Self::Unique(inner) => Self::Unique(Box::new(inner.substitute(bindings))),
            Self::Weak(inner) => Self::Weak(Box::new(inner.substitute(bindings))),
            Self::Address { pointee, space } => Self::Address {
                pointee: Box::new(pointee.substitute(bindings)),
                space: *space,
            },
            Self::Function(signature) => Self::Function(substitute_signature(signature, bindings)),
            Self::Closure(closure) => Self::Closure(ClosureType {
                borrowed: closure.borrowed,
                signature: substitute_signature(&closure.signature, bindings),
            }),
            Self::Coroutine {
                yield_type,
                resume_type,
                result_type,
            } => Self::Coroutine {
                yield_type: Box::new(yield_type.substitute(bindings)),
                resume_type: Box::new(resume_type.substitute(bindings)),
                result_type: Box::new(result_type.substitute(bindings)),
            },
            Self::Texture {
                dimension,
                component,
                access,
                multisampled,
            } => Self::Texture {
                dimension: *dimension,
                component: Box::new(component.substitute(bindings)),
                access: *access,
                multisampled: *multisampled,
            },
            Self::VisibleFunctionTable(signature) => {
                Self::VisibleFunctionTable(Box::new(substitute_signature(signature, bindings)))
            }
            Self::Tensor { count, element } => Self::Tensor {
                count: *count,
                element: Box::new(element.substitute(bindings)),
            },
            Self::CooperativeMatrix {
                scope,
                rows,
                columns,
                element,
                use_kind,
            } => Self::CooperativeMatrix {
                scope: *scope,
                rows: *rows,
                columns: *columns,
                element: Box::new(element.substitute(bindings)),
                use_kind: *use_kind,
            },
            Self::Entry {
                kind,
                root,
                immediate,
            } => Self::Entry {
                kind: *kind,
                root: Box::new(root.substitute(bindings)),
                immediate: Box::new(immediate.substitute(bindings)),
            },
            Self::Integer(_)
            | Self::Float(_)
            | Self::Pointer(_)
            | Self::Code
            | Self::Unit
            | Self::Never
            | Self::Frame(_)
            | Self::Sampler
            | Self::AccelerationStructure
            | Self::RayQuery
            | Self::HitObject => self.clone(),
        }
    }
}

fn substitute_signature(
    signature: &FunctionType,
    bindings: &HashMap<String, Type>,
) -> FunctionType {
    FunctionType {
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                context: parameter.context,
                convention: parameter.convention,
                ty: parameter.ty.substitute(bindings),
            })
            .collect(),
        result: Box::new(signature.result.substitute(bindings)),
        failure: signature
            .failure
            .as_ref()
            .map(|failure| Box::new(failure.substitute(bindings))),
        asynchronous: signature.asynchronous,
        abi: signature.abi,
        foreign: signature.foreign,
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(bits) => write!(formatter, "i{bits}"),
            Self::Float(kind) => fmt::Display::fmt(kind, formatter),
            Self::Pointer(AddressSpace::Generic) => formatter.write_str("ptr"),
            Self::Pointer(space) => write!(formatter, "ptr<{}>", space.name()),
            Self::Code => formatter.write_str("code"),
            Self::Unit => formatter.write_str("unit"),
            Self::Never => formatter.write_str("never"),
            Self::FixedVector { lanes, element } => {
                write!(formatter, "vec<{lanes} x {element}>")
            }
            Self::ScalableVector(element) => write!(formatter, "svec<{element}>"),
            Self::Tuple(elements) => {
                formatter.write_str("(")?;
                write_joined(formatter, elements)?;
                formatter.write_str(")")
            }
            Self::Array { count, element } => write!(formatter, "[{count} x {element}]"),
            Self::Nominal { path, arguments } => {
                write!(formatter, "%{path}")?;
                if !arguments.is_empty() {
                    formatter.write_str("<")?;
                    write_joined(formatter, arguments)?;
                    formatter.write_str(">")?;
                }
                Ok(())
            }
            Self::Generic(name) => write!(formatter, "${name}"),
            Self::Reference(inner) => write!(formatter, "ref {inner}"),
            Self::Unique(inner) => write!(formatter, "uniq {inner}"),
            Self::Weak(inner) => write!(formatter, "weak {inner}"),
            Self::Address { pointee, space } => {
                write!(formatter, "*{pointee}")?;
                if let Some(space) = space {
                    write!(formatter, "<{}>", space.name())?;
                }
                Ok(())
            }
            Self::Function(signature) => fmt::Display::fmt(signature, formatter),
            Self::Closure(closure) => {
                if closure.borrowed {
                    formatter.write_str("borrow ")?;
                }
                formatter.write_str("closure")?;
                closure.signature.fmt_signature(formatter)
            }
            Self::Coroutine {
                yield_type,
                resume_type,
                result_type,
            } => write!(
                formatter,
                "coro<{yield_type}, {resume_type}, {result_type}>"
            ),
            Self::Frame(path) => write!(formatter, "frame @{path}"),
            Self::Texture {
                dimension,
                component,
                access,
                multisampled,
            } => {
                write!(
                    formatter,
                    "texture<{}, {component}, {}",
                    dimension.name(),
                    access.name()
                )?;
                if *multisampled {
                    formatter.write_str(", ms")?;
                }
                formatter.write_str(">")
            }
            Self::Sampler => formatter.write_str("sampler"),
            Self::AccelerationStructure => formatter.write_str("accel"),
            Self::VisibleFunctionTable(signature) => write!(formatter, "vft<{signature}>"),
            Self::Tensor { count, element } => write!(formatter, "tensor<{count} x {element}>"),
            Self::RayQuery => formatter.write_str("rayquery"),
            Self::HitObject => formatter.write_str("hitobject"),
            Self::CooperativeMatrix {
                scope,
                rows,
                columns,
                element,
                use_kind,
            } => write!(
                formatter,
                "coop<{}, {rows} x {columns}, {element}, {}>",
                scope.name(),
                use_kind.name()
            ),
            Self::Entry {
                kind,
                root,
                immediate,
            } => write!(
                formatter,
                "entry<{}, root {root}, immediate {immediate}>",
                kind.name()
            ),
        }
    }
}

fn write_joined<T: fmt::Display>(formatter: &mut fmt::Formatter<'_>, values: &[T]) -> fmt::Result {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            formatter.write_str(", ")?;
        }
        fmt::Display::fmt(value, formatter)?;
    }
    Ok(())
}
