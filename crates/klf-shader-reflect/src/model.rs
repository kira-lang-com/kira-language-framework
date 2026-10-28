//! The reflection of one shader for one target (section 21.1).

use klf_shader::{Stage, Target};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reflection {
    pub shader_name: String,
    /// `graphics` or `compute`.
    pub shader_kind: String,
    pub target: Option<Target>,
    /// The producing definition, `ksl` for instance.
    pub source: String,
    pub options: Vec<OptionRecord>,
    pub stages: Vec<StageRecord>,
    pub resources: Vec<ResourceRecord>,
    pub types: Vec<TypeRecord>,
    pub renames: Vec<Rename>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionRecord {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub id: Option<u16>,
    /// Whether the target keeps it specializable; otherwise it was baked.
    pub keep: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageRecord {
    pub stage: Stage,
    pub entry_name: String,
    pub input_type: Option<String>,
    pub output_type: Option<String>,
    pub threads: Option<[u32; 3]>,
    pub inputs: Vec<FieldRecord>,
    pub outputs: Vec<FieldRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldRecord {
    pub name: String,
    pub type_name: String,
    pub builtin: Option<String>,
    pub interpolation: Option<String>,
    pub location: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindRecord {
    pub target: Target,
    pub group: u32,
    pub binding: u32,
    pub glsl_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceRecord {
    pub group_name: String,
    pub group_class: String,
    pub group_index: u32,
    pub name: String,
    /// `uniform`, `storage`, `texture` or `sampler`.
    pub kind: String,
    pub type_name: String,
    /// `read`, `read_write` or `write`.
    pub access: Option<String>,
    pub visibility: Vec<Stage>,
    pub bindings: Vec<BindRecord>,
    pub paired_sampler: Option<String>,
    pub lengths: Vec<(Target, u32)>,
}

impl ResourceRecord {
    #[must_use]
    pub fn binding(&self, target: Target) -> Option<&BindRecord> {
        self.bindings
            .iter()
            .find(|binding| binding.target == target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutField {
    pub name: String,
    pub offset: u32,
    pub alignment: u32,
    pub size: u32,
    pub stride: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub alignment: u32,
    pub size: u32,
    pub fields: Vec<LayoutField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeRecord {
    pub name: String,
    pub fields: Vec<FieldRecord>,
    pub layout: Option<Layout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    pub kind: String,
    pub from: String,
    pub to: String,
}
