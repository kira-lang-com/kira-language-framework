use crate::{Architecture, Platform};

#[path = "cpu_data.rs"]
mod data;

use self::data::{
    AARCH64_FEATURES, ANDROID_BASELINES, ANDROID_OPTIONAL, APPLE_BASELINES, LINUX_BASELINES,
    LINUX_OPTIONAL, RV64_FEATURES, TARGETS, TESSERA_BASELINES, TESSERA_OPTIONAL, WASM_BASELINES,
    WASM_FEATURES, WEB_OPTIONAL, WINDOWS_BASELINES, WINDOWS_OPTIONAL, X64_FEATURES,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SvecMode {
    Fixed(u8),
    Agnostic,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum CpuTarget {
    Aarch64Base,
    Aarch64V86,
    Aarch64V9,
    X64V3,
    X64V4,
    X64Avx10_2,
    X64Apx,
    Rv64Base,
    Rv64Zvk,
    Aarch64V80,
    Aarch64V82,
    Aarch64Apple1,
    X64V2,
    X64V3h,
    X64V4h,
    Rv64Rva23,
    WasmBase,
    WasmM64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuTargetInfo {
    pub target: CpuTarget,
    pub name: &'static str,
    pub architecture: Architecture,
    pub parent: Option<CpuTarget>,
    pub added_features: &'static [&'static str],
    pub svec: SvecMode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CpuModifier {
    Mte,
    Fpac,
    Gcs,
    Sme2,
    Amx,
}

impl CpuModifier {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "mte" => Some(Self::Mte),
            "fpac" => Some(Self::Fpac),
            "gcs" => Some(Self::Gcs),
            "sme2" => Some(Self::Sme2),
            "amx" => Some(Self::Amx),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mte => "mte",
            Self::Fpac => "fpac",
            Self::Gcs => "gcs",
            Self::Sme2 => "sme2",
            Self::Amx => "amx",
        }
    }

    #[must_use]
    pub const fn features(self) -> &'static [&'static str] {
        match self {
            Self::Mte => &["mte", "mte3"],
            Self::Fpac => &["fpac"],
            Self::Gcs => &["gcs"],
            Self::Sme2 => &["sme", "sme2"],
            Self::Amx => &["amx_tile", "amx_int8", "amx_bf16"],
        }
    }

    #[must_use]
    pub const fn architecture(self) -> Architecture {
        match self {
            Self::Mte | Self::Fpac | Self::Gcs | Self::Sme2 => Architecture::Aarch64,
            Self::Amx => Architecture::X64,
        }
    }
}

#[must_use]
pub const fn known_features(architecture: Architecture) -> &'static [&'static str] {
    match architecture {
        Architecture::Aarch64 => AARCH64_FEATURES,
        Architecture::X64 => X64_FEATURES,
        Architecture::Rv64 => RV64_FEATURES,
        Architecture::Wasm => WASM_FEATURES,
    }
}

pub use self::data::ALL_CPU_TARGETS;

impl CpuTarget {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_CPU_TARGETS
            .into_iter()
            .find(|target| target.info().name == name)
    }

    #[must_use]
    pub fn info(self) -> &'static CpuTargetInfo {
        &TARGETS[self as usize]
    }

    #[must_use]
    pub fn features(self) -> Vec<&'static str> {
        let mut lineage = Vec::new();
        let mut current = Some(self);
        while let Some(target) = current {
            let info = target.info();
            lineage.push(info);
            current = info.parent;
        }

        let mut features = Vec::new();
        while let Some(info) = lineage.pop() {
            for feature in info.added_features {
                if !features.contains(feature) {
                    features.push(*feature);
                }
            }
        }
        features
    }

    #[must_use]
    pub fn supports_feature(self, feature: &str) -> bool {
        let info = self.info();
        info.added_features.contains(&feature)
            || info
                .parent
                .is_some_and(|parent| parent.supports_feature(feature))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetSelection {
    pub target: CpuTarget,
    pub modifiers: Vec<CpuModifier>,
}

impl TargetSelection {
    #[must_use]
    pub const fn new(target: CpuTarget) -> Self {
        Self {
            target,
            modifiers: Vec::new(),
        }
    }

    pub fn add_modifier(&mut self, modifier: CpuModifier) -> Result<(), ModifierError> {
        if self.target.info().architecture != modifier.architecture() {
            return Err(ModifierError {
                target: self.target,
                modifier,
            });
        }
        if !self.modifiers.contains(&modifier) {
            self.modifiers.push(modifier);
        }
        Ok(())
    }

    #[must_use]
    pub fn supports_feature(&self, feature: &str) -> bool {
        self.target.supports_feature(feature)
            || self
                .modifiers
                .iter()
                .any(|modifier| modifier.features().contains(&feature))
    }

    #[must_use]
    pub fn features(&self) -> Vec<&'static str> {
        let mut features = self.target.features();
        for feature in self
            .modifiers
            .iter()
            .flat_map(|modifier| modifier.features())
        {
            if !features.contains(feature) {
                features.push(feature);
            }
        }
        features
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModifierError {
    pub target: CpuTarget,
    pub modifier: CpuModifier,
}

impl std::fmt::Display for ModifierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "modifier +{} is unavailable on {}",
            self.modifier.name(),
            self.target.info().name
        )
    }
}

impl std::error::Error for ModifierError {}

pub fn platform_baselines(platform: Platform) -> &'static [CpuTarget] {
    match platform {
        Platform::Portable => &[],
        Platform::Tessera => &TESSERA_BASELINES,
        Platform::Macos | Platform::Ios => &APPLE_BASELINES,
        Platform::Windows => &WINDOWS_BASELINES,
        Platform::Linux => &LINUX_BASELINES,
        Platform::Android => &ANDROID_BASELINES,
        Platform::Web | Platform::Wasi => &WASM_BASELINES,
    }
}

pub fn platform_optional_targets(platform: Platform) -> &'static [CpuTarget] {
    match platform {
        Platform::Portable | Platform::Macos | Platform::Ios | Platform::Wasi => &[],
        Platform::Tessera => &TESSERA_OPTIONAL,
        Platform::Windows => &WINDOWS_OPTIONAL,
        Platform::Linux => &LINUX_OPTIONAL,
        Platform::Android => &ANDROID_OPTIONAL,
        Platform::Web => &WEB_OPTIONAL,
    }
}
