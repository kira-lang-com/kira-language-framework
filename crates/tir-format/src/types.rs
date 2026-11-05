//! Decoded records from the TIR binary TYPES section.

use crate::leb::IntegerImmediate;
use tir_types::FloatType;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryTypeTable {
    pub declarations: Vec<BinaryNominalDeclaration>,
    pub entries: Vec<BinaryType>,
    pub definitions: Vec<BinaryNominalDefinition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryConstantTable {
    pub records: Vec<BinaryConstant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryGlobalTable {
    pub records: Vec<BinaryGlobal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySignatureTable {
    pub records: Vec<BinarySignature>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryCodeTable {
    pub functions: Vec<BinaryFunctionBody>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryFunctionBody {
    pub function_index: u64,
    pub body_byte_size: u64,
    pub block_count: u64,
    pub blocks: Vec<BinaryBlock>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryBlock {
    pub cold: bool,
    pub loop_merge: Option<u64>,
    pub loop_continue: Option<u64>,
    pub parameters: Vec<BinaryBlockParameter>,
    pub instruction_count: u64,
    pub instruction_bytes: Vec<u8>,
    /// `None` means this reader does not yet support an opcode in the block.
    pub instructions: Option<Vec<BinaryInstruction>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryInstruction {
    pub offset: u64,
    pub opcode: u64,
    pub modifiers: u64,
    pub result_count: u64,
    pub terminator: bool,
    pub operands: Vec<BinaryOperand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryOperand {
    Value(u64),
    Values(Vec<u64>),
    Type(u64),
    Types(Vec<u64>),
    Index(u64),
    Indices(Vec<u64>),
    Symbol { kind: u8, index: u64 },
    Integer(IntegerImmediate),
    Integers(Vec<IntegerImmediate>),
    OptionalFloat32(Option<u32>),
    Bytes(Vec<u8>),
    OptionalValue(Option<u64>),
    BlockTarget { block: u64, arguments: Vec<u64> },
    Clauses(Vec<(u8, u64)>),
    SwitchCases(Vec<(IntegerImmediate, u64, Vec<u64>)>),
    SwitchEnumCases(Vec<(u64, u64)>),
    OptionalIndex(Option<u64>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryBlockParameter {
    /// 0 owned, 1 borrow, 2 borrowmut, 3 trivial, or 4 omitted/default.
    pub convention: u8,
    pub context: bool,
    pub ty: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySignature {
    pub name: String,
    pub linkage: BinaryLinkage,
    pub function_type: u64,
    pub generic_parameters: Vec<BinaryGenericParameter>,
    pub modifiers: u64,
    pub structured: bool,
    pub kernel_fp: bool,
    pub returns_twice: bool,
    pub inlinable: bool,
    pub engine: Option<BinaryEngine>,
    pub semantics: Option<crate::SemanticsMode>,
    pub target: BinaryTarget,
    pub fp_mode: Option<BinaryFpMode>,
    pub generic_strategy: Option<BinaryGenericStrategy>,
    pub coroutine: Option<(u64, u64)>,
    pub instance_types: Vec<u64>,
    pub variant: Option<BinaryVariantAttribute>,
    pub entry_kind: Option<BinaryEntryKind>,
    pub platform_export: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryVariantAttribute {
    pub architecture: u8,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryVariantSet {
    pub name: String,
    pub members: Vec<BinaryVariantMember>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryGpuTable {
    pub entries: Vec<BinaryGpuEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryGpuEntry {
    pub function_index: u64,
    pub kind: BinaryEntryKind,
    pub threads: [u64; 3],
    pub interfaces: Vec<BinaryGpuInterface>,
    pub payload_size: Option<u64>,
    pub threadgroup_memory: u64,
    pub mesh_maximum_vertices: u64,
    pub mesh_maximum_primitives: u64,
    pub mesh_topology: u8,
    pub capabilities: u64,
    pub subgroup_size: u64,
    pub floating_point_mode: Option<BinaryFpMode>,
    pub specialization_constants: Vec<BinaryGpuSpecializationConstant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryGpuInterface {
    pub role: BinaryGpuInterfaceRole,
    pub parameter_index: Option<u64>,
    pub ty: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryGpuInterfaceRole {
    Root,
    Immediate,
    StageParameter,
    IntersectionAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryGpuSpecializationConstant {
    pub id: u32,
    pub ty: u64,
    pub default_constant: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryVariantMember {
    pub architecture: u8,
    pub features: Vec<String>,
    pub function_index: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryEngine {
    Native,
    Runtime,
    Any,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryTarget {
    Cpu,
    Gpu,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryFpMode {
    Precise,
    Fast,
    IeeeDenormals,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryGenericStrategy {
    Specialize,
    Polymorphic,
    Both,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryEntryKind {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryGlobal {
    pub name: String,
    pub linkage: BinaryLinkage,
    pub ty: u64,
    pub address_space: BinaryAddressSpace,
    pub thread_local: bool,
    pub constant: bool,
    pub per_cpu: bool,
    pub alignment: Option<u64>,
    pub initializer: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryLinkage {
    Private,
    Module,
    Package,
    Exported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryAddressSpace {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryConstant {
    pub ty: u64,
    pub value: BinaryConstantValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinaryConstantValue {
    Integer(Vec<u8>),
    Float(Vec<u8>),
    Zero,
    Bytes(Vec<u8>),
    String(String),
    Tuple(Vec<BinaryConstantValue>),
    Array(Vec<BinaryConstantValue>),
    Vector(Vec<BinaryConstantValue>),
    Struct(Vec<BinaryConstantValue>),
    Union {
        member: u64,
        value: Box<BinaryConstantValue>,
    },
    Enum {
        variant: u64,
        payload: Vec<BinaryConstantValue>,
    },
    Null,
    Sampler([u8; 23]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryNominalKind {
    Struct,
    Enum,
    Explicit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryNominalDeclaration {
    pub name: String,
    pub generic_parameters: Vec<BinaryGenericParameter>,
    pub kind: BinaryNominalKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryGenericParameter {
    pub name: String,
    pub bounds: Vec<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum BinaryType {
    Integer(u64),
    Float(FloatType),
    Pointer(u8),
    Address {
        pointee: u64,
        space: u8,
    },
    Unit,
    Never,
    FixedVector {
        lanes: u64,
        element: u64,
    },
    ScalableVector {
        element: u64,
    },
    Tuple(Vec<u64>),
    Array {
        count: u64,
        element: u64,
    },
    Nominal {
        declaration: u64,
        arguments: Vec<u64>,
    },
    Generic {
        parameter: u64,
    },
    Reference {
        pointee: u64,
    },
    Unique {
        pointee: u64,
    },
    Weak {
        pointee: u64,
    },
    Function(BinaryFunctionType),
    Closure {
        function_type: u64,
        borrowed: bool,
    },
    Coroutine {
        yield_type: u64,
        resume_type: u64,
        result_type: u64,
    },
    Frame {
        function_index: u64,
    },
    Texture {
        dimension: u8,
        component: u64,
        access: u8,
        multisampled: bool,
    },
    Sampler,
    AccelerationStructure,
    VisibleFunctionTable {
        function_type: u64,
    },
    Tensor {
        rank: u64,
        element: u64,
    },
    RayQuery,
    HitObject,
    CooperativeMatrix {
        scope: u8,
        rows: u64,
        columns: u64,
        element: u64,
        use_kind: u8,
    },
    Entry {
        kind: u8,
        root: u64,
        immediate: u64,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BinaryFunctionType {
    pub parameters: Vec<BinaryFunctionParameter>,
    pub result: u64,
    pub error: Option<u64>,
    pub asynchronous: bool,
    pub c_abi: bool,
    pub foreign: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BinaryFunctionParameter {
    pub convention: u8,
    pub context: bool,
    pub ty: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryNominalDefinition {
    pub declaration: u64,
    pub layout: BinaryLayout,
    pub fields: Vec<BinaryField>,
    pub variants: Vec<BinaryVariant>,
    pub policy: BinaryPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryLayout {
    Ridl,
    C,
    Packed(u64),
    Explicit { size: u64, align: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryField {
    pub name: String,
    pub ty: u64,
    pub offset: Option<u64>,
    pub bit_offset: Option<u64>,
    pub bit_width: Option<u64>,
    pub annotations: Vec<BinaryAnnotation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAnnotation {
    pub name: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryVariant {
    pub name: String,
    pub payload: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryPolicy {
    pub copy: BinaryCopyPolicy,
    pub destroy: BinaryDestroyPolicy,
    pub relocate: BinaryRelocatePolicy,
    pub exclusivity: u8,
    pub pairing: u8,
    pub lifetime: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryCopyPolicy {
    Trivial,
    Implicit(u64),
    Explicit(u64),
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryDestroyPolicy {
    Trivial,
    Function(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryRelocatePolicy {
    Bitwise,
    Function(u64),
}

impl Default for BinaryTypeTable {
    fn default() -> Self {
        Self {
            declarations: Vec::new(),
            entries: Vec::new(),
            definitions: Vec::new(),
        }
    }
}

impl Default for BinaryConstantTable {
    fn default() -> Self {
        Self {
            records: Vec::new(),
        }
    }
}

impl Default for BinaryGlobalTable {
    fn default() -> Self {
        Self {
            records: Vec::new(),
        }
    }
}

impl Default for BinarySignatureTable {
    fn default() -> Self {
        Self {
            records: Vec::new(),
        }
    }
}

impl Default for BinaryCodeTable {
    fn default() -> Self {
        Self {
            functions: Vec::new(),
        }
    }
}
