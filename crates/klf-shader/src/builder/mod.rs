//! The shader builder (section 6.3): what a definition's `lower` calls. It
//! interns types, applies the layout defaults, keeps `Emit` ranges
//! (section 6.4) and checks every operand eagerly, reporting misuse as a
//! definition bug.

use crate::Span;
use crate::arena::Handle;
use crate::ir::Expression;
use crate::layout::{self, MemberSpec};
use crate::module::{
    Constant, EntryPoint, FunctionArgument, FunctionResult, GlobalVariable, GroupDecl, Module,
    Override, ResourceBinding, ShaderDecl, ShaderKind, Stage, TargetSlot, WorkgroupExtent,
};
use crate::resolve::{Context, Resolution};
use crate::types::{
    AddressSpace, ArraySize, ImageClass, ImageDim, LayoutRule, Scalar, StructMember, Type,
    TypeInner,
};

/// A builder misuse: the definition's bug, `KLF-SHD-9001`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, BuildError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(BuildError(message.into()))
}

/// The layout rule per host-shared space (section 5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutDefaults {
    pub uniform: LayoutRule,
    pub storage: LayoutRule,
    pub immediate: LayoutRule,
}

impl LayoutDefaults {
    #[must_use]
    pub const fn rule(self, space: AddressSpace) -> LayoutRule {
        match space {
            AddressSpace::Uniform => self.uniform,
            AddressSpace::Storage { .. } => self.storage,
            AddressSpace::Immediate => self.immediate,
            _ => LayoutRule::None,
        }
    }
}

pub struct ModuleBuilder {
    pub module: Module,
    pub layouts: LayoutDefaults,
    global_resolutions: Vec<Resolution>,
}

impl ModuleBuilder {
    #[must_use]
    pub fn new(name: impl Into<String>, lang: impl Into<String>, layouts: LayoutDefaults) -> Self {
        Self {
            module: Module::new(name, lang),
            layouts,
            global_resolutions: Vec::new(),
        }
    }

    pub fn intern(&mut self, inner: TypeInner, span: Span) -> Handle<Type> {
        self.module.types.insert(Type { name: None, inner }, span)
    }

    /// # Errors
    /// Fails on a width the kind does not have.
    pub fn scalar_type(&mut self, scalar: Scalar) -> Result<Handle<Type>> {
        check_scalar(scalar)?;
        Ok(self.intern(TypeInner::Scalar(scalar), Span::default()))
    }

    /// # Errors
    /// Fails on a size outside 2 to 4.
    pub fn vector_type(&mut self, size: u8, scalar: Scalar) -> Result<Handle<Type>> {
        check_scalar(scalar)?;
        if !(2..=4).contains(&size) {
            return fail(format!("a vector has 2 to 4 components, not {size}"));
        }
        Ok(self.intern(TypeInner::Vector { size, scalar }, Span::default()))
    }

    /// # Errors
    /// Fails on a dimension outside 2 to 4 or a non-float scalar.
    pub fn matrix_type(&mut self, columns: u8, rows: u8, scalar: Scalar) -> Result<Handle<Type>> {
        if !(2..=4).contains(&columns) || !(2..=4).contains(&rows) {
            return fail("a matrix has 2 to 4 columns and rows");
        }
        if scalar.kind != crate::types::ScalarKind::Float {
            return fail("a matrix holds floats");
        }
        Ok(self.intern(
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            },
            Span::default(),
        ))
    }

    /// # Errors
    /// Fails unless the scalar is a 32-bit integer.
    pub fn atomic_type(&mut self, scalar: Scalar) -> Result<Handle<Type>> {
        if !scalar.kind.is_integer() || scalar.width != 4 {
            return fail("an atomic holds a 32-bit integer");
        }
        Ok(self.intern(TypeInner::Atomic(scalar), Span::default()))
    }

    /// An array of `base` with the stride the layout rule of `space` gives.
    ///
    /// # Errors
    /// Fails on a zero size.
    pub fn array_type(
        &mut self,
        base: Handle<Type>,
        size: ArraySize,
        space: AddressSpace,
    ) -> Result<Handle<Type>> {
        self.type_exists(base)?;
        if size == ArraySize::Constant(0) {
            return fail("an array has at least one element");
        }
        let stride = layout::array_stride(&self.module.types, base, self.layouts.rule(space));
        Ok(self.array_type_strided(base, size, stride))
    }

    pub fn array_type_strided(
        &mut self,
        base: Handle<Type>,
        size: ArraySize,
        stride: u32,
    ) -> Handle<Type> {
        self.intern(TypeInner::Array { base, size, stride }, Span::default())
    }

    /// A struct laid out by the rule `space` takes.
    ///
    /// # Errors
    /// Fails when that rule is `explicit`: the definition supplies offsets.
    pub fn struct_type(
        &mut self,
        name: &str,
        members: Vec<MemberSpec>,
        space: AddressSpace,
        span: Span,
    ) -> Result<Handle<Type>> {
        let rule = self.layouts.rule(space);
        if rule == LayoutRule::Explicit {
            return fail(format!(
                "struct `{name}` is reached from a space whose layout is explicit; use structTypeExplicit"
            ));
        }
        for member in &members {
            self.type_exists(member.ty)?;
        }
        let (members, size, align) = layout::place_members(&self.module.types, members, rule);
        Ok(self.module.types.insert(
            Type {
                name: Some(name.to_owned()),
                inner: TypeInner::Struct {
                    rule,
                    members,
                    size,
                    align,
                },
            },
            span,
        ))
    }

    /// A struct whose offsets the definition computed (section 6.5).
    ///
    /// # Errors
    /// Fails on an unknown member type.
    pub fn struct_type_explicit(
        &mut self,
        name: &str,
        members: Vec<StructMember>,
        size: u32,
        align: u32,
        span: Span,
    ) -> Result<Handle<Type>> {
        for member in &members {
            self.type_exists(member.ty)?;
        }
        Ok(self.module.types.insert(
            Type {
                name: Some(name.to_owned()),
                inner: TypeInner::Struct {
                    rule: LayoutRule::Explicit,
                    members,
                    size,
                    align,
                },
            },
            span,
        ))
    }

    /// A struct with rule `none`: one that never reaches host-shared memory.
    ///
    /// # Errors
    /// Fails on an unknown member type.
    pub fn struct_type_plain(
        &mut self,
        name: &str,
        members: Vec<MemberSpec>,
        span: Span,
    ) -> Result<Handle<Type>> {
        self.struct_type(name, members, AddressSpace::Function, span)
    }

    /// # Errors
    /// Fails on an arrayed 3D image.
    pub fn image_type(
        &mut self,
        dim: ImageDim,
        arrayed: bool,
        class: ImageClass,
    ) -> Result<Handle<Type>> {
        if arrayed && dim == ImageDim::D3 {
            return fail("a 3D image cannot be arrayed");
        }
        Ok(self.intern(
            TypeInner::Image {
                dim,
                arrayed,
                class,
            },
            Span::default(),
        ))
    }

    pub fn sampler_type(&mut self, comparison: bool) -> Handle<Type> {
        self.intern(TypeInner::Sampler { comparison }, Span::default())
    }

    /// # Errors
    /// Fails on an unknown base type.
    pub fn pointer_type(
        &mut self,
        base: Handle<Type>,
        space: AddressSpace,
    ) -> Result<Handle<Type>> {
        self.type_exists(base)?;
        Ok(self.intern(TypeInner::Pointer { base, space }, Span::default()))
    }

    #[must_use]
    pub fn extent(&self, ty: Handle<Type>, rule: LayoutRule) -> (u32, u32) {
        layout::extent(&self.module.types, ty, rule)
    }

    fn type_exists(&self, ty: Handle<Type>) -> Result<()> {
        if self.module.types.get(ty).is_none() {
            return fail(format!("type {ty:?} does not exist"));
        }
        Ok(())
    }

    /// Appends a constant or override expression.
    ///
    /// # Errors
    /// Fails on an expression that cannot be global or has no type.
    pub fn global_expression(
        &mut self,
        expression: Expression,
        span: Span,
    ) -> Result<Handle<Expression>> {
        if !matches!(
            expression,
            Expression::Literal(_)
                | Expression::Constant(_)
                | Expression::Override(_)
                | Expression::ZeroValue(_)
                | Expression::Compose { .. }
                | Expression::Access { .. }
                | Expression::AccessIndex { .. }
                | Expression::Splat { .. }
                | Expression::Swizzle { .. }
                | Expression::Unary { .. }
                | Expression::Binary { .. }
                | Expression::Select { .. }
                | Expression::Relational { .. }
                | Expression::Math { .. }
                | Expression::Convert { .. }
                | Expression::Bitcast { .. }
        ) {
            return fail("only constant and override expressions are global");
        }
        let context = Context {
            module: &self.module,
            function: None,
            expressions: &self.module.global_expressions,
        };
        let resolution = context
            .resolve(&expression, &self.global_resolutions)
            .map_err(BuildError)?;
        self.global_resolutions.push(resolution);
        Ok(self.module.global_expressions.append(expression, span))
    }

    /// # Errors
    /// Fails when the initializer's type is not `ty`.
    pub fn constant(
        &mut self,
        name: &str,
        ty: Handle<Type>,
        init: Handle<Expression>,
        span: Span,
    ) -> Result<Handle<Constant>> {
        self.check_global_init(ty, init)?;
        Ok(self.module.constants.append(
            Constant {
                name: name.to_owned(),
                ty,
                init,
            },
            span,
        ))
    }

    /// # Errors
    /// Fails when the default's type is not `ty`.
    pub fn override_(
        &mut self,
        name: &str,
        id: Option<u16>,
        ty: Handle<Type>,
        init: Option<Handle<Expression>>,
        span: Span,
    ) -> Result<Handle<Override>> {
        if let Some(init) = init {
            self.check_global_init(ty, init)?;
        }
        Ok(self.module.overrides.append(
            Override {
                name: name.to_owned(),
                id,
                ty,
                init,
            },
            span,
        ))
    }

    fn check_global_init(&self, ty: Handle<Type>, init: Handle<Expression>) -> Result<()> {
        let resolution = self
            .global_resolutions
            .get(init.index())
            .ok_or_else(|| BuildError(format!("global expression {init:?} does not exist")))?;
        if !resolution.same(&Resolution::Handle(ty), &self.module.types) {
            return fail("an initializer's type differs from the declared type");
        }
        Ok(())
    }

    /// # Errors
    /// Fails on an unknown type.
    pub fn global(
        &mut self,
        name: &str,
        space: AddressSpace,
        ty: Handle<Type>,
        binding: Option<ResourceBinding>,
        span: Span,
    ) -> Result<Handle<GlobalVariable>> {
        self.type_exists(ty)?;
        Ok(self.module.globals.append(
            GlobalVariable {
                name: name.to_owned(),
                space,
                ty,
                binding,
                slots: Vec::new(),
                init: None,
            },
            span,
        ))
    }

    /// # Errors
    /// Fails on an unknown global.
    pub fn authored_slot(
        &mut self,
        global: Handle<GlobalVariable>,
        slot: TargetSlot,
    ) -> Result<()> {
        let global = self
            .module
            .globals
            .get_mut(global)
            .ok_or_else(|| BuildError("unknown global".to_owned()))?;
        global.slots.push(slot);
        Ok(())
    }

    /// Starts a function. It joins the arena when finished.
    #[must_use]
    pub fn function(
        &self,
        name: &str,
        arguments: Vec<FunctionArgument>,
        result: Option<FunctionResult>,
        span: Span,
    ) -> FunctionBuilder {
        FunctionBuilder::new(name, arguments, result, None, span)
    }

    #[must_use]
    pub fn entry_point(
        &self,
        name: &str,
        stage: Stage,
        arguments: Vec<FunctionArgument>,
        result: Option<FunctionResult>,
        span: Span,
    ) -> FunctionBuilder {
        FunctionBuilder::new(name, arguments, result, Some(stage), span)
    }

    /// Adds a finished function to the arena.
    ///
    /// # Errors
    /// Fails on an entry point's builder.
    pub fn add_function(
        &mut self,
        builder: FunctionBuilder,
    ) -> Result<Handle<crate::module::Function>> {
        if builder.stage.is_some() {
            return fail("an entry point is added with add_entry_point");
        }
        let span = builder.span;
        let function = builder.finish()?;
        Ok(self.module.functions.append(function, span))
    }

    /// Adds a finished entry point and returns its index.
    ///
    /// # Errors
    /// Fails on a plain function's builder or a compute entry without a size.
    pub fn add_entry_point(
        &mut self,
        builder: FunctionBuilder,
        workgroup_size: Option<[WorkgroupExtent; 3]>,
    ) -> Result<usize> {
        let Some(stage) = builder.stage else {
            return fail("a function is added with add_function");
        };
        if (stage == Stage::Compute) != workgroup_size.is_some() {
            return fail("a compute entry, and only one, declares a workgroup size");
        }
        let span = builder.span;
        let name = builder.function.name.clone();
        let function = builder.finish()?;
        self.module.entry_points.push(EntryPoint {
            name,
            stage,
            workgroup_size,
            early_depth_test: None,
            function,
            span,
        });
        Ok(self.module.entry_points.len() - 1)
    }

    /// # Errors
    /// Fails on an entry point or global that does not exist.
    pub fn shader(
        &mut self,
        name: &str,
        kind: ShaderKind,
        entries: Vec<usize>,
        groups: Vec<GroupDecl>,
        options: Vec<Handle<Override>>,
        span: Span,
    ) -> Result<()> {
        if entries
            .iter()
            .any(|entry| *entry >= self.module.entry_points.len())
        {
            return fail("a shader names an entry point that does not exist");
        }
        if groups
            .iter()
            .flat_map(|group| &group.globals)
            .any(|global| self.module.globals.get(*global).is_none())
        {
            return fail("a shader group names a global that does not exist");
        }
        self.module.shaders.push(ShaderDecl {
            name: name.to_owned(),
            kind,
            entries,
            groups,
            options,
            span,
        });
        Ok(())
    }

    #[must_use]
    pub fn finish(self) -> Module {
        self.module
    }
}

fn check_scalar(scalar: Scalar) -> Result<()> {
    use crate::types::ScalarKind;
    let valid = match scalar.kind {
        ScalarKind::Bool => scalar.width == 1,
        ScalarKind::Sint | ScalarKind::Uint => matches!(scalar.width, 4 | 8),
        ScalarKind::Float => matches!(scalar.width, 2 | 4 | 8),
    };
    if valid {
        Ok(())
    } else {
        fail(format!(
            "no {} scalar is {} bytes wide",
            scalar.kind.name(),
            scalar.width
        ))
    }
}
mod function;
pub use function::FunctionBuilder;
