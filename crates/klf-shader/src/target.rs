//! The targets the kernel and the bundled outs write (section 19.2).

use crate::ir::named_enum;

named_enum!(Target {
    Msl => "msl",
    Spirv => "spirv",
    Hlsl => "hlsl",
    Glsl => "glsl_430",
    Wgsl => "wgsl",
    Ksl => "ksl",
});

impl Target {
    /// Whether the target compiles one unit per entry point.
    #[must_use]
    pub const fn per_stage(self) -> bool {
        !matches!(self, Self::Msl | Self::Ksl)
    }

    /// The extension of the unit for a stage, without the stage suffix.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Msl => "metal",
            Self::Spirv => "spv",
            Self::Hlsl => "hlsl",
            Self::Glsl => "glsl",
            Self::Wgsl => "wgsl",
            Self::Ksl => "ksl",
        }
    }
}
