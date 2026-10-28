//! The `ShaderLanguage` family (`KLF-KSL.md` section 5): what a shader
//! definition declares beyond the members it shares with `Language`, and
//! what its phases read from the Session.

use klf_shader::builder::LayoutDefaults;
use klf_shader::{Capability, Target};

/// The members only a `ShaderLanguage` declares (section 5.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderFamily {
    pub layouts: LayoutDefaults,
    pub targets: Vec<Target>,
    /// Shared shader modules shipped with the toolchain, relative to the
    /// package; empty for none.
    pub library_root: String,
    /// The language's code for each kernel diagnostic key (section 16.4).
    pub keys: Vec<(String, i64)>,
    /// What every module the language lowers may use (section 16.6).
    pub capabilities: Vec<Capability>,
}

/// Every key a shader language maps (`KLF-KSL.md` section 16.4).
pub const SHADER_KEYS: &[&str] = &[
    "shader.uniformity",
    "shader.const-not-finite",
    "shader.const-index",
    "shader.interface-mismatch",
    "shader.override-missing",
    "shader.override-unsupported",
    "shader.target-unsupported",
    "shader.capability",
    "shader.slot-collision",
];

/// What the shader builder needs from the Session: the language name every
/// module records, the layout rule of each host-shared space and the
/// capabilities every module declares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderSettings {
    pub lang: String,
    pub layouts: LayoutDefaults,
    pub capabilities: Vec<Capability>,
}

impl crate::Session {
    /// The shader language this session compiles, when it is one.
    #[must_use]
    pub fn shader(&self) -> Option<&ShaderSettings> {
        self.shader.as_ref()
    }

    pub fn set_shader(&mut self, settings: ShaderSettings) {
        self.shader = Some(settings);
    }
}
