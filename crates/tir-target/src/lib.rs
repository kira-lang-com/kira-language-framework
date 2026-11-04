#![forbid(unsafe_code)]

mod cpu;
mod gpu;
mod tir64;

pub use cpu::{
    ALL_CPU_TARGETS, CpuModifier, CpuTarget, CpuTargetInfo, ModifierError, SvecMode,
    TargetSelection, known_features, platform_baselines, platform_optional_targets,
};
pub use gpu::{ALL_GPU_FAMILIES, ALL_HOSTED_GPU_TARGETS, GpuFamily, HostedGpuTarget};
pub use tir64::{
    CScalar, CScalarLayout, TIR64, TIR64_C_DATA_LAYOUT, TIR64_POINTER_WIDTH, Tir64Model,
    bitint_layout,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Profile {
    App = 0,
    System = 1,
    Driver = 2,
    Kernel = 3,
}

impl Profile {
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::App),
            1 => Some(Self::System),
            2 => Some(Self::Driver),
            3 => Some(Self::Kernel),
            _ => None,
        }
    }

    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::System => "system",
            Self::Driver => "driver",
            Self::Kernel => "kernel",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Platform {
    Portable = 0,
    Tessera = 1,
    Macos = 2,
    Ios = 3,
    Windows = 4,
    Linux = 5,
    Android = 6,
    Web = 7,
    Wasi = 8,
}

impl Platform {
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Portable),
            1 => Some(Self::Tessera),
            2 => Some(Self::Macos),
            3 => Some(Self::Ios),
            4 => Some(Self::Windows),
            5 => Some(Self::Linux),
            6 => Some(Self::Android),
            7 => Some(Self::Web),
            8 => Some(Self::Wasi),
            _ => None,
        }
    }

    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Tessera => "tessera",
            Self::Macos => "macos",
            Self::Ios => "ios",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Android => "android",
            Self::Web => "web",
            Self::Wasi => "wasi",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Architecture {
    Aarch64,
    X64,
    Rv64,
    Wasm,
}

impl Architecture {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64",
            Self::X64 => "x64",
            Self::Rv64 => "rv64",
            Self::Wasm => "wasm",
        }
    }
}
