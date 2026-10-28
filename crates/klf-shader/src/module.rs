//! The KSIR module and its module-scope declarations (sections 7, 9, 10,
//! 13 and 14).

use crate::Span;
use crate::arena::{Arena, Handle, UniqueArena};
use crate::ir::{Block, Expression, named_enum};
use crate::types::{AddressSpace, Binding, Capability, Type};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    pub name: String,
    pub ty: Handle<Type>,
    pub init: Handle<Expression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub name: String,
    pub id: Option<u16>,
    pub ty: Handle<Type>,
    pub init: Option<Handle<Expression>>,
}

/// A resource's `(group, binding)`, the same on every target (section 9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceBinding {
    pub group: u32,
    pub binding: u32,
}

named_enum!(
    /// The class of an HLSL register.
    HlslClass {
        B => "b",
        T => "t",
        U => "u",
        S => "s",
    }
);

/// A target slot written in source (section 9.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetSlot {
    Universal(u32),
    MslBuffer(u32),
    MslTexture(u32),
    MslSampler(u32),
    Hlsl {
        class: HlslClass,
        register: u32,
        space: u32,
    },
    Glsl(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalVariable {
    pub name: String,
    pub space: AddressSpace,
    pub ty: Handle<Type>,
    pub binding: Option<ResourceBinding>,
    pub slots: Vec<TargetSlot>,
    pub init: Option<Handle<Expression>>,
}

impl GlobalVariable {
    #[must_use]
    pub fn universal_slot(&self) -> Option<u32> {
        self.slots.iter().find_map(|slot| match slot {
            TargetSlot::Universal(value) => Some(*value),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalVariable {
    pub name: Option<String>,
    pub ty: Handle<Type>,
    pub init: Option<Handle<Expression>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionArgument {
    pub name: Option<String>,
    pub ty: Handle<Type>,
    pub binding: Option<Binding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionResult {
    pub ty: Handle<Type>,
    pub binding: Option<Binding>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Function {
    pub name: String,
    pub arguments: Vec<FunctionArgument>,
    pub result: Option<FunctionResult>,
    pub locals: Arena<LocalVariable>,
    pub expressions: Arena<Expression>,
    pub named_expressions: BTreeMap<u32, String>,
    pub body: Block,
}

named_enum!(Stage {
    Vertex => "vertex",
    Fragment => "fragment",
    Compute => "compute",
});

impl Stage {
    /// The file suffix of a per-stage output (section 4.5).
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Vertex => "vert",
            Self::Fragment => "frag",
            Self::Compute => "comp",
        }
    }
}

/// One workgroup extent: a number or an override expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkgroupExtent {
    Fixed(u32),
    Override(Handle<Expression>),
}

named_enum!(EarlyDepthTest {
    Force => "force",
    GreaterEqual => "greater_equal",
    LessEqual => "less_equal",
    Unchanged => "unchanged",
});

#[derive(Clone, Debug, PartialEq)]
pub struct EntryPoint {
    pub name: String,
    pub stage: Stage,
    pub workgroup_size: Option<[WorkgroupExtent; 3]>,
    pub early_depth_test: Option<EarlyDepthTest>,
    pub function: Function,
    pub span: Span,
}

named_enum!(ShaderKind {
    Graphics => "graphics",
    Compute => "compute",
});

named_enum!(GroupClass {
    Frame => "frame",
    Pass => "pass",
    Material => "material",
    Object => "object",
    Draw => "draw",
    Dispatch => "dispatch",
    Custom => "custom",
});

#[derive(Clone, Debug, PartialEq)]
pub struct GroupDecl {
    pub name: String,
    pub class: GroupClass,
    pub globals: Vec<Handle<GlobalVariable>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShaderDecl {
    pub name: String,
    pub kind: ShaderKind,
    pub entries: Vec<usize>,
    pub groups: Vec<GroupDecl>,
    pub options: Vec<Handle<Override>>,
    pub span: Span,
}

/// A KSIR module (section 7.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub name: String,
    pub lang: String,
    pub capabilities: BTreeSet<Capability>,
    pub types: UniqueArena<Type>,
    pub global_expressions: Arena<Expression>,
    pub constants: Arena<Constant>,
    pub overrides: Arena<Override>,
    pub globals: Arena<GlobalVariable>,
    pub functions: Arena<Function>,
    pub entry_points: Vec<EntryPoint>,
    pub shaders: Vec<ShaderDecl>,
    /// The source files spans name, by index.
    pub files: Vec<String>,
}

impl Module {
    #[must_use]
    pub fn new(name: impl Into<String>, lang: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lang: lang.into(),
            ..Self::default()
        }
    }

    /// The index of `path` in `files`, added when new.
    pub fn file(&mut self, path: &str) -> u32 {
        if let Some(index) = self.files.iter().position(|file| file == path) {
            return index as u32;
        }
        self.files.push(path.to_owned());
        (self.files.len() - 1) as u32
    }

    /// The function of a function handle or an entry point.
    #[must_use]
    pub fn function(&self, which: FunctionRef) -> &Function {
        match which {
            FunctionRef::Function(handle) => &self.functions[handle],
            FunctionRef::Entry(index) => &self.entry_points[index].function,
        }
    }

    pub fn function_mut(&mut self, which: FunctionRef) -> &mut Function {
        match which {
            FunctionRef::Function(handle) => &mut self.functions[handle],
            FunctionRef::Entry(index) => &mut self.entry_points[index].function,
        }
    }

    /// Every function and then every entry point.
    #[must_use]
    pub fn function_refs(&self) -> Vec<FunctionRef> {
        self.functions
            .iter()
            .map(|(handle, _)| FunctionRef::Function(handle))
            .chain((0..self.entry_points.len()).map(FunctionRef::Entry))
            .collect()
    }

    /// The shader an entry point belongs to.
    #[must_use]
    pub fn shader_of_entry(&self, entry: usize) -> Option<&ShaderDecl> {
        self.shaders
            .iter()
            .find(|shader| shader.entries.contains(&entry))
    }
}

/// A function of the arena or an entry point's function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionRef {
    Function(Handle<Function>),
    Entry(usize),
}
