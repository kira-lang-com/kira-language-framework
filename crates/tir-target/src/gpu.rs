#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GpuFamily {
    TesseraTvgpu,
    AmdGfx103,
    AmdGfx110,
    AmdGfx115,
    AmdGfx120,
    NvidiaSm75,
    NvidiaSm80,
    NvidiaSm86,
    NvidiaSm89,
    NvidiaSm90,
    NvidiaSm100,
    NvidiaSm120,
    IntelXeHpg,
    IntelXeLpg,
    IntelXe2,
    IntelXe3,
    QualcommA7xx,
    QualcommA8xx,
    ArmValhallCsf,
    ArmGen5,
    ImaginationD,
    ImaginationE,
    AppleG13,
    AppleG14,
    AppleG15,
    AppleG16,
}

pub const ALL_GPU_FAMILIES: [GpuFamily; 26] = [
    GpuFamily::TesseraTvgpu,
    GpuFamily::AmdGfx103,
    GpuFamily::AmdGfx110,
    GpuFamily::AmdGfx115,
    GpuFamily::AmdGfx120,
    GpuFamily::NvidiaSm75,
    GpuFamily::NvidiaSm80,
    GpuFamily::NvidiaSm86,
    GpuFamily::NvidiaSm89,
    GpuFamily::NvidiaSm90,
    GpuFamily::NvidiaSm100,
    GpuFamily::NvidiaSm120,
    GpuFamily::IntelXeHpg,
    GpuFamily::IntelXeLpg,
    GpuFamily::IntelXe2,
    GpuFamily::IntelXe3,
    GpuFamily::QualcommA7xx,
    GpuFamily::QualcommA8xx,
    GpuFamily::ArmValhallCsf,
    GpuFamily::ArmGen5,
    GpuFamily::ImaginationD,
    GpuFamily::ImaginationE,
    GpuFamily::AppleG13,
    GpuFamily::AppleG14,
    GpuFamily::AppleG15,
    GpuFamily::AppleG16,
];

impl GpuFamily {
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Self::TesseraTvgpu => 0x0001,
            Self::AmdGfx103 => 0x0100,
            Self::AmdGfx110 => 0x0101,
            Self::AmdGfx115 => 0x0102,
            Self::AmdGfx120 => 0x0103,
            Self::NvidiaSm75 => 0x0200,
            Self::NvidiaSm80 => 0x0201,
            Self::NvidiaSm86 => 0x0202,
            Self::NvidiaSm89 => 0x0203,
            Self::NvidiaSm90 => 0x0204,
            Self::NvidiaSm100 => 0x0205,
            Self::NvidiaSm120 => 0x0206,
            Self::IntelXeHpg => 0x0300,
            Self::IntelXeLpg => 0x0301,
            Self::IntelXe2 => 0x0302,
            Self::IntelXe3 => 0x0303,
            Self::QualcommA7xx => 0x0400,
            Self::QualcommA8xx => 0x0401,
            Self::ArmValhallCsf => 0x0500,
            Self::ArmGen5 => 0x0501,
            Self::ImaginationD => 0x0600,
            Self::ImaginationE => 0x0601,
            Self::AppleG13 => 0x0700,
            Self::AppleG14 => 0x0701,
            Self::AppleG15 => 0x0702,
            Self::AppleG16 => 0x0703,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TesseraTvgpu => "tessera.tvgpu",
            Self::AmdGfx103 => "amd.gfx103",
            Self::AmdGfx110 => "amd.gfx110",
            Self::AmdGfx115 => "amd.gfx115",
            Self::AmdGfx120 => "amd.gfx120",
            Self::NvidiaSm75 => "nvidia.sm75",
            Self::NvidiaSm80 => "nvidia.sm80",
            Self::NvidiaSm86 => "nvidia.sm86",
            Self::NvidiaSm89 => "nvidia.sm89",
            Self::NvidiaSm90 => "nvidia.sm90",
            Self::NvidiaSm100 => "nvidia.sm100",
            Self::NvidiaSm120 => "nvidia.sm120",
            Self::IntelXeHpg => "intel.xe_hpg",
            Self::IntelXeLpg => "intel.xe_lpg",
            Self::IntelXe2 => "intel.xe2",
            Self::IntelXe3 => "intel.xe3",
            Self::QualcommA7xx => "qcom.a7xx",
            Self::QualcommA8xx => "qcom.a8xx",
            Self::ArmValhallCsf => "arm.valhall_csf",
            Self::ArmGen5 => "arm.gen5",
            Self::ImaginationD => "img.d",
            Self::ImaginationE => "img.e",
            Self::AppleG13 => "apple.g13",
            Self::AppleG14 => "apple.g14",
            Self::AppleG15 => "apple.g15",
            Self::AppleG16 => "apple.g16",
        }
    }

    #[must_use]
    pub fn from_code(code: u16) -> Option<Self> {
        ALL_GPU_FAMILIES
            .into_iter()
            .find(|family| family.code() == code)
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_GPU_FAMILIES
            .into_iter()
            .find(|family| family.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostedGpuTarget {
    Msl,
    Dxil,
    Spirv,
    Wgsl,
}

pub const ALL_HOSTED_GPU_TARGETS: [HostedGpuTarget; 4] = [
    HostedGpuTarget::Msl,
    HostedGpuTarget::Dxil,
    HostedGpuTarget::Spirv,
    HostedGpuTarget::Wgsl,
];

impl HostedGpuTarget {
    #[must_use]
    pub const fn code(self) -> u16 {
        match self {
            Self::Msl => 0x0800,
            Self::Dxil => 0x0801,
            Self::Spirv => 0x0802,
            Self::Wgsl => 0x0803,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Msl => "host.msl",
            Self::Dxil => "host.dxil",
            Self::Spirv => "host.spirv",
            Self::Wgsl => "host.wgsl",
        }
    }

    #[must_use]
    pub fn from_code(code: u16) -> Option<Self> {
        ALL_HOSTED_GPU_TARGETS
            .into_iter()
            .find(|target| target.code() == code)
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_HOSTED_GPU_TARGETS
            .into_iter()
            .find(|target| target.name() == name)
    }
}
