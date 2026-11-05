use tir_diag::Span;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum DataKind {
    Readonly,
    Relro,
    Writable,
    Zero,
    Tls,
    TlsZero,
    GpuConstant,
    Unknown,
}

impl DataKind {
    pub(super) fn parse(name: &str) -> Self {
        match name {
            "readonly" => Self::Readonly,
            "relro" => Self::Relro,
            "writable" => Self::Writable,
            "zero" => Self::Zero,
            "tls" => Self::Tls,
            "tls_zero" => Self::TlsZero,
            "gpu_constant" => Self::GpuConstant,
            _ => Self::Unknown,
        }
    }

    pub(super) const fn is_zero_initialized(self) -> bool {
        matches!(self, Self::Zero | Self::TlsZero)
    }
}

#[derive(Clone, Copy)]
pub(super) enum TargetKind {
    Data(DataKind),
    DataImport,
    DefinedFunction {
        gpu: bool,
        address_taken: bool,
        tag: Option<u32>,
    },
    ImportedFunction {
        tag: Option<u32>,
    },
    VariantSet,
    DispatchSet,
    GpuEntry,
}

#[derive(Clone, Copy)]
pub(super) enum RelocationKind {
    Abs64,
    Code64,
    Rel32,
    GpuEntry64,
    Tpoff64,
    Import64,
    ImportCode64,
    Dispatch64,
}

impl RelocationKind {
    pub(super) fn parse(name: &str) -> Option<Self> {
        match name {
            "ABS64" => Some(Self::Abs64),
            "CODE64" => Some(Self::Code64),
            "REL32" => Some(Self::Rel32),
            "GPUENTRY64" => Some(Self::GpuEntry64),
            "TPOFF64" => Some(Self::Tpoff64),
            "IMPORT64" => Some(Self::Import64),
            "IMPORTCODE64" => Some(Self::ImportCode64),
            "DISPATCH64" => Some(Self::Dispatch64),
            _ => None,
        }
    }

    pub(super) const fn width(self) -> u64 {
        match self {
            Self::Rel32 => 4,
            Self::Abs64
            | Self::Code64
            | Self::GpuEntry64
            | Self::Tpoff64
            | Self::Import64
            | Self::ImportCode64
            | Self::Dispatch64 => 8,
        }
    }
}

pub(super) struct Relocation {
    pub(super) kind: RelocationKind,
    pub(super) offset: u64,
    pub(super) target: String,
    pub(super) tag: Option<u32>,
    pub(super) target_span: Span,
    pub(super) span: Span,
}

pub(super) struct DataObject {
    pub(super) kind: DataKind,
    pub(super) per_cpu: bool,
    pub(super) size: Option<u64>,
    pub(super) relocations: Vec<Relocation>,
}
