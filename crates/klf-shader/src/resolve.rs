//! The type of every expression (section 11.1).

use crate::arena::{Arena, Handle, UniqueArena};
use crate::ir::{BinaryOp, Expression, ImageQuery, MathFunction, RelationalFunction};
use crate::module::{Function, Module};
use crate::types::{AddressSpace, ImageClass, ImageDim, Scalar, ScalarKind, Type, TypeInner};

/// An expression's type: a handle into the type arena, or a shape that may
/// not be in it.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    Handle(Handle<Type>),
    Value(TypeInner),
}

impl Resolution {
    #[must_use]
    pub fn inner<'a>(&'a self, types: &'a UniqueArena<Type>) -> &'a TypeInner {
        match self {
            Self::Handle(handle) => &types[*handle].inner,
            Self::Value(inner) => inner,
        }
    }

    /// The type handle, interning the shape when it is new.
    pub fn handle(&self, types: &mut UniqueArena<Type>) -> Handle<Type> {
        match self {
            Self::Handle(handle) => *handle,
            Self::Value(inner) => types.insert(
                Type {
                    name: None,
                    inner: inner.clone(),
                },
                crate::Span::default(),
            ),
        }
    }

    /// Whether two resolutions name the same type.
    #[must_use]
    pub fn same(&self, other: &Self, types: &UniqueArena<Type>) -> bool {
        match (self, other) {
            (Self::Handle(left), Self::Handle(right)) => left == right,
            _ => self.inner(types) == other.inner(types),
        }
    }
}

/// The handle of an unnamed type with `inner`, when the arena has one.
#[must_use]
pub fn find(types: &UniqueArena<Type>, inner: &TypeInner) -> Option<Handle<Type>> {
    types.find(&Type {
        name: None,
        inner: inner.clone(),
    })
}

/// The context expressions resolve in: the module, and the function whose
/// arguments, locals and earlier expressions they may name.
pub struct Context<'a> {
    pub module: &'a Module,
    pub function: Option<&'a Function>,
    pub expressions: &'a Arena<Expression>,
}

impl Context<'_> {
    fn of(resolved: &[Resolution], handle: Handle<Expression>) -> Result<Resolution, String> {
        resolved
            .get(handle.index())
            .cloned()
            .ok_or_else(|| format!("expression {handle:?} is used before it is defined"))
    }

    /// The type of `expression`, given the types of every earlier one.
    ///
    /// # Errors
    /// Returns why the expression has no type.
    pub fn resolve(
        &self,
        expression: &Expression,
        resolved: &[Resolution],
    ) -> Result<Resolution, String> {
        let types = &self.module.types;
        let inner_of = |handle| -> Result<TypeInner, String> {
            Ok(Self::of(resolved, handle)?.inner(types).clone())
        };
        Ok(match expression {
            Expression::Literal(literal) => Resolution::Value(TypeInner::Scalar(literal.scalar())),
            Expression::Constant(constant) => Resolution::Handle(
                self.module
                    .constants
                    .get(*constant)
                    .ok_or("unknown constant")?
                    .ty,
            ),
            Expression::Override(item) => Resolution::Handle(
                self.module
                    .overrides
                    .get(*item)
                    .ok_or("unknown override")?
                    .ty,
            ),
            Expression::ZeroValue(ty)
            | Expression::Compose { ty, .. }
            | Expression::Bitcast { ty, .. } => Resolution::Handle(*ty),
            Expression::AtomicResult { ty } | Expression::WorkgroupUniformLoadResult { ty } => {
                Resolution::Handle(*ty)
            }
            Expression::Access { base, .. } => self.element(&inner_of(*base)?, None)?,
            Expression::AccessIndex { base, index } => {
                self.element(&inner_of(*base)?, Some(*index))?
            }
            Expression::Splat { size, value } => {
                let scalar = inner_of(*value)?.scalar().ok_or("splat of a non-scalar")?;
                Resolution::Value(TypeInner::Vector {
                    size: *size,
                    scalar,
                })
            }
            Expression::Swizzle { vector, pattern } => {
                let TypeInner::Vector { scalar, .. } = inner_of(*vector)? else {
                    return Err("swizzle of a non-vector".to_owned());
                };
                match pattern.len() {
                    1 => Resolution::Value(TypeInner::Scalar(scalar)),
                    size => Resolution::Value(TypeInner::Vector {
                        size: size as u8,
                        scalar,
                    }),
                }
            }
            Expression::FunctionArgument(index) => Resolution::Handle(
                self.function
                    .and_then(|function| function.arguments.get(*index as usize))
                    .ok_or("argument index out of range")?
                    .ty,
            ),
            Expression::GlobalVariable(global) => {
                let global = self.module.globals.get(*global).ok_or("unknown global")?;
                match global.space {
                    AddressSpace::Handle => Resolution::Handle(global.ty),
                    space => Resolution::Value(TypeInner::Pointer {
                        base: global.ty,
                        space,
                    }),
                }
            }
            Expression::LocalVariable(local) => {
                let local = self
                    .function
                    .and_then(|function| function.locals.get(*local))
                    .ok_or("unknown local")?;
                Resolution::Value(TypeInner::Pointer {
                    base: local.ty,
                    space: AddressSpace::Function,
                })
            }
            Expression::Load { pointer } => match inner_of(*pointer)? {
                TypeInner::Pointer { base, .. } => match &types[base].inner {
                    TypeInner::Atomic(scalar) => Resolution::Value(TypeInner::Scalar(*scalar)),
                    _ => Resolution::Handle(base),
                },
                _ => return Err("load through a non-pointer".to_owned()),
            },
            Expression::ImageSample {
                image,
                gather,
                depth_ref,
                ..
            } => {
                let class = image_class(&inner_of(*image)?)?.1;
                match (class, depth_ref, gather) {
                    (_, Some(_), None) | (ImageClass::Depth { .. }, None, None) => {
                        Resolution::Value(TypeInner::Scalar(Scalar::F32))
                    }
                    (ImageClass::Sampled { kind, .. }, _, _) => vec4(kind),
                    _ => vec4(ScalarKind::Float),
                }
            }
            Expression::ImageLoad { image, .. } => match image_class(&inner_of(*image)?)?.1 {
                ImageClass::Sampled { kind, .. } => vec4(kind),
                ImageClass::Depth { .. } => Resolution::Value(TypeInner::Scalar(Scalar::F32)),
                ImageClass::Storage { format, .. } => vec4(format.kind()),
            },
            Expression::ImageQuery { image, query } => match query {
                ImageQuery::Size { .. } => {
                    let (dim, _) = image_class(&inner_of(*image)?)?;
                    match dim {
                        ImageDim::D1 => Resolution::Value(TypeInner::Scalar(Scalar::U32)),
                        ImageDim::D2 | ImageDim::Cube => vector(2, Scalar::U32),
                        ImageDim::D3 => vector(3, Scalar::U32),
                    }
                }
                ImageQuery::NumLevels | ImageQuery::NumLayers | ImageQuery::NumSamples => {
                    Resolution::Value(TypeInner::Scalar(Scalar::U32))
                }
            },
            Expression::Unary { expr, .. } | Expression::Derivative { expr, .. } => {
                Self::of(resolved, *expr)?
            }
            Expression::Select { accept, .. } => Self::of(resolved, *accept)?,
            Expression::Binary { op, left, right } => self.binary(*op, *left, *right, resolved)?,
            Expression::Relational { fun, argument } => match fun {
                RelationalFunction::All | RelationalFunction::Any => {
                    Resolution::Value(TypeInner::Scalar(Scalar::BOOL))
                }
                RelationalFunction::IsNan | RelationalFunction::IsInf => {
                    match inner_of(*argument)? {
                        TypeInner::Vector { size, .. } => vector(size, Scalar::BOOL),
                        _ => Resolution::Value(TypeInner::Scalar(Scalar::BOOL)),
                    }
                }
            },
            Expression::Math { fun, args } => {
                let first = args.first().ok_or("math with no arguments")?;
                Self::math(*fun, &inner_of(*first)?, Self::of(resolved, *first)?)?
            }
            Expression::Convert { expr, kind, width } => {
                let (size, scalar) = inner_of(*expr)?
                    .scalar_or_vector()
                    .ok_or("conversion of a non-numeric value")?;
                let width = match kind {
                    ScalarKind::Bool => 1,
                    _ => width.unwrap_or(if scalar.width == 1 { 4 } else { scalar.width }),
                };
                let scalar = Scalar { kind: *kind, width };
                match size {
                    Some(size) => vector(size, scalar),
                    None => Resolution::Value(TypeInner::Scalar(scalar)),
                }
            }
            Expression::CallResult(function) => {
                let function = self
                    .module
                    .functions
                    .get(*function)
                    .ok_or("unknown function")?;
                Resolution::Handle(
                    function
                        .result
                        .as_ref()
                        .ok_or("call result of a void function")?
                        .ty,
                )
            }
            Expression::ArrayLength(_) => Resolution::Value(TypeInner::Scalar(Scalar::U32)),
        })
    }

    fn element(&self, base: &TypeInner, index: Option<u32>) -> Result<Resolution, String> {
        let types = &self.module.types;
        let (pointee, space) = match base {
            TypeInner::Pointer { base, space } => (&types[*base].inner, Some(*space)),
            other => (other, None),
        };
        let element = match pointee {
            TypeInner::Vector { scalar, .. } => Resolution::Value(TypeInner::Scalar(*scalar)),
            TypeInner::Matrix { rows, scalar, .. } => Resolution::Value(TypeInner::Vector {
                size: *rows,
                scalar: *scalar,
            }),
            TypeInner::Array { base, .. } => Resolution::Handle(*base),
            TypeInner::Struct { members, .. } => {
                let index = index.ok_or("a struct is indexed by constant member index only")?;
                Resolution::Handle(
                    members
                        .get(index as usize)
                        .ok_or("struct member index out of range")?
                        .ty,
                )
            }
            _ => return Err("indexing a value that has no elements".to_owned()),
        };
        Ok(match space {
            None => element,
            Some(space) => {
                let base = match element {
                    Resolution::Handle(handle) => handle,
                    Resolution::Value(inner) => find(types, &inner)
                        .ok_or("the element type of a pointer access is not in the type arena")?,
                };
                Resolution::Value(TypeInner::Pointer { base, space })
            }
        })
    }

    fn binary(
        &self,
        op: BinaryOp,
        left: Handle<Expression>,
        right: Handle<Expression>,
        resolved: &[Resolution],
    ) -> Result<Resolution, String> {
        let types = &self.module.types;
        let left_type = Self::of(resolved, left)?;
        let right_type = Self::of(resolved, right)?;
        let (left_inner, right_inner) = (left_type.inner(types), right_type.inner(types));
        if op.is_comparison() {
            return Ok(match left_inner {
                TypeInner::Vector { size, .. } => vector(*size, Scalar::BOOL),
                _ => Resolution::Value(TypeInner::Scalar(Scalar::BOOL)),
            });
        }
        if op == BinaryOp::Multiply {
            match (left_inner, right_inner) {
                (TypeInner::Matrix { rows, scalar, .. }, TypeInner::Matrix { columns, .. }) => {
                    return Ok(Resolution::Value(TypeInner::Matrix {
                        columns: *columns,
                        rows: *rows,
                        scalar: *scalar,
                    }));
                }
                (TypeInner::Matrix { rows, scalar, .. }, TypeInner::Vector { .. }) => {
                    return Ok(vector(*rows, *scalar));
                }
                (
                    TypeInner::Vector { .. },
                    TypeInner::Matrix {
                        columns, scalar, ..
                    },
                ) => {
                    return Ok(vector(*columns, *scalar));
                }
                (TypeInner::Scalar(_), TypeInner::Matrix { .. } | TypeInner::Vector { .. }) => {
                    return Ok(right_type);
                }
                _ => {}
            }
        }
        if matches!(left_inner, TypeInner::Scalar(_))
            && matches!(right_inner, TypeInner::Vector { .. })
        {
            return Ok(right_type);
        }
        Ok(left_type)
    }

    fn math(
        fun: MathFunction,
        first: &TypeInner,
        resolution: Resolution,
    ) -> Result<Resolution, String> {
        let scalar = first.scalar().unwrap_or(Scalar::F32);
        Ok(match fun {
            MathFunction::Dot
            | MathFunction::Length
            | MathFunction::Distance
            | MathFunction::Determinant => Resolution::Value(TypeInner::Scalar(scalar)),
            MathFunction::Cross => vector(3, scalar),
            MathFunction::Transpose => match first {
                TypeInner::Matrix {
                    columns,
                    rows,
                    scalar,
                } => Resolution::Value(TypeInner::Matrix {
                    columns: *rows,
                    rows: *columns,
                    scalar: *scalar,
                }),
                _ => return Err("transpose of a non-matrix".to_owned()),
            },
            MathFunction::Pack4x8Snorm
            | MathFunction::Pack4x8Unorm
            | MathFunction::Pack2x16Snorm
            | MathFunction::Pack2x16Unorm
            | MathFunction::Pack2x16Float => Resolution::Value(TypeInner::Scalar(Scalar::U32)),
            MathFunction::Unpack4x8Snorm | MathFunction::Unpack4x8Unorm => vector(4, Scalar::F32),
            MathFunction::Unpack2x16Snorm
            | MathFunction::Unpack2x16Unorm
            | MathFunction::Unpack2x16Float => vector(2, Scalar::F32),
            _ => resolution,
        })
    }
}

fn vector(size: u8, scalar: Scalar) -> Resolution {
    Resolution::Value(TypeInner::Vector { size, scalar })
}

fn vec4(kind: ScalarKind) -> Resolution {
    vector(4, Scalar { kind, width: 4 })
}

fn image_class(inner: &TypeInner) -> Result<(ImageDim, ImageClass), String> {
    match inner {
        TypeInner::Image { dim, class, .. } => Ok((*dim, *class)),
        _ => Err("an image operation on a value that is not an image".to_owned()),
    }
}

/// The type of every expression of `function`, in arena order.
///
/// # Errors
/// Returns the first expression that has no type, with why.
pub fn resolve_function(
    module: &Module,
    function: &Function,
) -> Result<Vec<Resolution>, (Handle<Expression>, String)> {
    let context = Context {
        module,
        function: Some(function),
        expressions: &function.expressions,
    };
    resolve_arena(&context)
}

/// The type of every global expression.
///
/// # Errors
/// Returns the first expression that has no type, with why.
pub fn resolve_globals(module: &Module) -> Result<Vec<Resolution>, (Handle<Expression>, String)> {
    let context = Context {
        module,
        function: None,
        expressions: &module.global_expressions,
    };
    resolve_arena(&context)
}

fn resolve_arena(context: &Context<'_>) -> Result<Vec<Resolution>, (Handle<Expression>, String)> {
    let mut resolved = Vec::with_capacity(context.expressions.len());
    for (handle, expression) in context.expressions.iter() {
        let resolution = context
            .resolve(expression, &resolved)
            .map_err(|message| (handle, message))?;
        resolved.push(resolution);
    }
    Ok(resolved)
}
