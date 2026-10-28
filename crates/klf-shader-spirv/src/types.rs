//! Types and constants: plain types for values, and decorated layout
//! copies of the aggregates buffers hold (section 19.4 rule 5).

use crate::Writer;
use crate::words::{capability, decoration, op};
use klf_shader::layout::matrix_extent;
use klf_shader::{
    AddressSpace, ArraySize, Expression, Handle, ImageClass, ImageDim, LayoutRule, Literal, Scalar,
    ScalarKind, Span, StorageFormat, Type, TypeInner,
};
use klf_shader_valid::{ConstScalar, ConstValue};

/// What a type id is cached under. Aggregates have a plain and a layout
/// form; every other type has one id, as SPIR-V requires.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum TypeKey {
    Void,
    Bool,
    Scalar(Scalar),
    Vector(u8, Scalar),
    Matrix(u8, u8, Scalar),
    Image(ImageDim, bool, ImageClass),
    Sampler,
    SampledImage(u32),
    Aggregate(Option<String>, TypeInner, bool),
    Wrapper(Handle<Type>, bool),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum ConstKey {
    Words(u32, Vec<u32>),
    Bool(bool),
    Composite(u32, Vec<u32>),
    Null(u32),
}

impl Writer<'_> {
    fn cached(&mut self, key: TypeKey, make: impl FnOnce(&mut Self, u32)) -> u32 {
        if let Some(id) = self.types.get(&key) {
            return *id;
        }
        let id = self.id();
        make(self, id);
        self.types.insert(key, id);
        id
    }

    pub(crate) fn void(&mut self) -> u32 {
        self.cached(TypeKey::Void, |writer, id| {
            writer.declare(op::TYPE_VOID, &[id]);
        })
    }

    pub(crate) fn scalar(&mut self, scalar: Scalar) -> u32 {
        if scalar.kind == ScalarKind::Bool {
            return self.cached(TypeKey::Bool, |writer, id| {
                writer.declare(op::TYPE_BOOL, &[id]);
            });
        }
        self.cached(TypeKey::Scalar(scalar), |writer, id| {
            let bits = u32::from(scalar.width) * 8;
            match (scalar.kind, bits) {
                (ScalarKind::Float, 16) => {
                    writer.capabilities.insert(capability::FLOAT16);
                }
                (ScalarKind::Float, 64) => {
                    writer.capabilities.insert(capability::FLOAT64);
                }
                (_, 64) => {
                    writer.capabilities.insert(capability::INT64);
                }
                _ => {}
            }
            match scalar.kind {
                ScalarKind::Float => writer.declare(op::TYPE_FLOAT, &[id, bits]),
                kind => writer.declare(
                    op::TYPE_INT,
                    &[id, bits, u32::from(kind == ScalarKind::Sint)],
                ),
            }
        })
    }

    pub(crate) fn vector(&mut self, size: u8, scalar: Scalar) -> u32 {
        let component = self.scalar(scalar);
        self.cached(TypeKey::Vector(size, scalar), |writer, id| {
            writer.declare(op::TYPE_VECTOR, &[id, component, u32::from(size)]);
        })
    }

    /// A scalar, or a vector of `size` when there is one.
    pub(crate) fn scalar_or_vector(&mut self, size: Option<u8>, scalar: Scalar) -> u32 {
        match size {
            Some(size) => self.vector(size, scalar),
            None => self.scalar(scalar),
        }
    }

    /// The id of a type handle, in its layout form when `layout`.
    pub(crate) fn type_of(&mut self, ty: Handle<Type>, layout: bool) -> u32 {
        let module = self.module;
        let ty = &module.types[ty];
        self.type_inner(&ty.inner, ty.name.as_deref(), layout)
    }

    pub(crate) fn type_inner(
        &mut self,
        inner: &TypeInner,
        name: Option<&str>,
        layout: bool,
    ) -> u32 {
        let module = self.module;
        match inner {
            TypeInner::Scalar(scalar) | TypeInner::Atomic(scalar) => self.scalar(*scalar),
            TypeInner::Vector { size, scalar } => self.vector(*size, *scalar),
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            } => {
                let column = self.vector(*rows, *scalar);
                self.cached(TypeKey::Matrix(*columns, *rows, *scalar), |writer, id| {
                    writer.declare(op::TYPE_MATRIX, &[id, column, u32::from(*columns)]);
                })
            }
            TypeInner::Pointer { base, space } => {
                let pointee = self.type_of(*base, space.is_host_shared());
                self.pointer(crate::storage_class(*space), pointee)
            }
            TypeInner::Image {
                dim,
                arrayed,
                class,
            } => self.image(*dim, *arrayed, *class),
            TypeInner::Sampler { .. } => self.cached(TypeKey::Sampler, |writer, id| {
                writer.declare(op::TYPE_SAMPLER, &[id]);
            }),
            TypeInner::Array { base, size, stride } => {
                let element = self.type_of(*base, layout);
                let length = match size {
                    ArraySize::Constant(count) => Some(self.literal(Literal::U32(*count))),
                    ArraySize::Runtime => None,
                    ArraySize::Override(item) => Some(self.spec_constant(*item)),
                };
                let key = TypeKey::Aggregate(None, inner.clone(), layout);
                let stride = *stride;
                self.cached(key, |writer, id| {
                    match length {
                        Some(length) => writer.declare(op::TYPE_ARRAY, &[id, element, length]),
                        None => writer.declare(op::TYPE_RUNTIME_ARRAY, &[id, element]),
                    }
                    if layout {
                        writer.decorate(id, decoration::ARRAY_STRIDE, &[stride]);
                    }
                })
            }
            TypeInner::Struct { rule, members, .. } => {
                let layout = layout && *rule != LayoutRule::None;
                let key = TypeKey::Aggregate(name.map(str::to_owned), inner.clone(), layout);
                if let Some(id) = self.types.get(&key) {
                    return *id;
                }
                let member_ids: Vec<u32> = members
                    .iter()
                    .map(|member| self.type_of(member.ty, layout))
                    .collect();
                let id = self.id();
                let mut operands = vec![id];
                operands.extend(&member_ids);
                self.declare(op::TYPE_STRUCT, &operands);
                if let Some(name) = name {
                    self.name(id, name);
                }
                for (index, member) in members.iter().enumerate() {
                    let mut name_operands = vec![id, index as u32];
                    name_operands.extend(crate::words::string(&member.name));
                    self.debug
                        .extend(crate::words::instruction(op::MEMBER_NAME, &name_operands));
                    if layout {
                        self.member_decorate(
                            id,
                            index as u32,
                            decoration::OFFSET,
                            &[member.offset],
                        );
                        if let Some(stride) = matrix_stride(module, member.ty, member.size) {
                            self.member_decorate(id, index as u32, decoration::COL_MAJOR, &[]);
                            self.member_decorate(
                                id,
                                index as u32,
                                decoration::MATRIX_STRIDE,
                                &[stride],
                            );
                        }
                    }
                }
                self.types.insert(key, id);
                id
            }
        }
    }

    pub(crate) fn member_decorate(&mut self, id: u32, member: u32, decoration: u32, extra: &[u32]) {
        let mut operands = vec![id, member, decoration];
        operands.extend_from_slice(extra);
        self.annotations
            .extend(crate::words::instruction(op::MEMBER_DECORATE, &operands));
    }

    /// The block struct around a buffer global whose type is not a struct.
    pub(crate) fn wrapper(&mut self, ty: Handle<Type>, space: AddressSpace) -> u32 {
        let member = self.type_of(ty, true);
        let module = self.module;
        let key = TypeKey::Wrapper(ty, space == AddressSpace::Uniform);
        if let Some(id) = self.types.get(&key) {
            return *id;
        }
        let id = self.id();
        self.declare(op::TYPE_STRUCT, &[id, member]);
        self.decorate(id, decoration::BLOCK, &[]);
        self.member_decorate(id, 0, decoration::OFFSET, &[0]);
        let rule = if space == AddressSpace::Uniform {
            LayoutRule::Std140
        } else {
            LayoutRule::Std430
        };
        let size = match module.types[ty].inner {
            TypeInner::Matrix {
                columns,
                rows,
                scalar,
            } => matrix_extent(columns, rows, scalar.width, rule).0,
            _ => 0,
        };
        if let Some(stride) = matrix_stride(module, ty, size) {
            self.member_decorate(id, 0, decoration::COL_MAJOR, &[]);
            self.member_decorate(id, 0, decoration::MATRIX_STRIDE, &[stride]);
        }
        self.types.insert(key, id);
        id
    }

    pub(crate) fn pointer(&mut self, storage: u32, pointee: u32) -> u32 {
        if let Some(id) = self.pointers.get(&(storage, pointee)) {
            return *id;
        }
        let id = self.id();
        self.declare(op::TYPE_POINTER, &[id, storage, pointee]);
        self.pointers.insert((storage, pointee), id);
        id
    }

    pub(crate) fn sampled_image(&mut self, image: u32) -> u32 {
        self.cached(TypeKey::SampledImage(image), |writer, id| {
            writer.declare(op::TYPE_SAMPLED_IMAGE, &[id, image]);
        })
    }

    fn image(&mut self, dim: ImageDim, arrayed: bool, class: ImageClass) -> u32 {
        let (kind, depth, multisampled, sampled, format) = match class {
            ImageClass::Sampled { kind, multisampled } => (kind, 0, multisampled, 1, 0),
            ImageClass::Depth { multisampled } => (ScalarKind::Float, 1, multisampled, 1, 0),
            ImageClass::Storage { format, .. } => (format.kind(), 0, false, 2, self.format(format)),
        };
        let sampled_type = self.scalar(Scalar { kind, width: 4 });
        let spirv_dim = match dim {
            ImageDim::D1 => 0,
            ImageDim::D2 => 1,
            ImageDim::D3 => 2,
            ImageDim::Cube => 3,
        };
        match (dim, arrayed, sampled) {
            (ImageDim::D1, _, 1) => {
                self.capabilities.insert(capability::SAMPLED_1D);
            }
            (ImageDim::D1, _, _) => {
                self.capabilities.insert(capability::IMAGE_1D);
            }
            (ImageDim::Cube, true, 1) => {
                self.capabilities.insert(capability::SAMPLED_CUBE_ARRAY);
            }
            (ImageDim::Cube, true, _) => {
                self.capabilities.insert(capability::IMAGE_CUBE_ARRAY);
            }
            _ => {}
        }
        if multisampled && sampled == 2 {
            self.capabilities
                .insert(capability::STORAGE_IMAGE_MULTISAMPLE);
        }
        self.cached(TypeKey::Image(dim, arrayed, class), |writer, id| {
            writer.declare(
                op::TYPE_IMAGE,
                &[
                    id,
                    sampled_type,
                    spirv_dim,
                    depth,
                    u32::from(arrayed),
                    u32::from(multisampled),
                    sampled,
                    format,
                ],
            );
        })
    }

    fn format(&mut self, format: StorageFormat) -> u32 {
        let (value, extended) = match format {
            StorageFormat::Rgba32Float => (1, false),
            StorageFormat::Rgba16Float => (2, false),
            StorageFormat::R32Float => (3, false),
            StorageFormat::Rgba8Unorm => (4, false),
            StorageFormat::Rgba8Snorm => (5, false),
            StorageFormat::Rg32Float => (6, true),
            StorageFormat::Rg16Float => (7, true),
            StorageFormat::Rg11b10Ufloat => (8, true),
            StorageFormat::R16Float => (9, true),
            StorageFormat::Rgb10a2Unorm => (11, true),
            StorageFormat::Rg8Unorm => (13, true),
            StorageFormat::R8Unorm => (15, true),
            StorageFormat::Rgba32Sint => (21, false),
            StorageFormat::Rgba16Sint => (22, false),
            StorageFormat::Rgba8Sint => (23, false),
            StorageFormat::R32Sint => (24, false),
            StorageFormat::Rg32Sint => (25, true),
            StorageFormat::Rgba32Uint => (30, false),
            StorageFormat::Rgba16Uint => (31, false),
            StorageFormat::Rgba8Uint => (32, false),
            StorageFormat::R32Uint => (33, false),
            StorageFormat::Rg32Uint => (35, true),
            StorageFormat::Bgra8Unorm => {
                self.refuse("SPIR-V has no bgra8unorm storage format", Span::default());
                (0, false)
            }
        };
        if extended {
            self.capabilities
                .insert(capability::STORAGE_IMAGE_EXTENDED_FORMATS);
        }
        value
    }

    pub(crate) fn function_type(&mut self, result: u32, parameters: &[u32]) -> u32 {
        let mut key = vec![result];
        key.extend_from_slice(parameters);
        if let Some(id) = self.function_types.get(&key) {
            return *id;
        }
        let id = self.id();
        let mut operands = vec![id];
        operands.extend(&key);
        self.declare(op::TYPE_FUNCTION, &operands);
        self.function_types.insert(key, id);
        id
    }

    fn constant(&mut self, key: ConstKey) -> u32 {
        if let Some(id) = self.constants.get(&key) {
            return *id;
        }
        let id = self.id();
        match &key {
            ConstKey::Words(ty, words) => {
                let mut operands = vec![*ty, id];
                operands.extend(words);
                self.declare(op::CONSTANT, &operands);
            }
            ConstKey::Bool(value) => {
                let ty = self.scalar(Scalar::BOOL);
                let opcode = if *value {
                    op::CONSTANT_TRUE
                } else {
                    op::CONSTANT_FALSE
                };
                self.declare(opcode, &[ty, id]);
            }
            ConstKey::Composite(ty, parts) => {
                let mut operands = vec![*ty, id];
                operands.extend(parts);
                self.declare(op::CONSTANT_COMPOSITE, &operands);
            }
            ConstKey::Null(ty) => self.declare(op::CONSTANT_NULL, &[*ty, id]),
        }
        self.constants.insert(key, id);
        id
    }

    pub(crate) fn null(&mut self, ty: u32) -> u32 {
        self.constant(ConstKey::Null(ty))
    }

    pub(crate) fn composite_constant(&mut self, ty: u32, parts: Vec<u32>) -> u32 {
        self.constant(ConstKey::Composite(ty, parts))
    }

    pub(crate) fn literal(&mut self, literal: Literal) -> u32 {
        let ty = self.scalar(literal.scalar());
        if let Literal::Bool(value) = literal {
            return self.constant(ConstKey::Bool(value));
        }
        self.constant(ConstKey::Words(ty, literal_words(literal)))
    }

    /// A scalar literal of `scalar` with an integer value, splatted to
    /// `size` components when given.
    pub(crate) fn number(&mut self, scalar: Scalar, value: i16, size: Option<u8>) -> u32 {
        let literal = match (scalar.kind, scalar.width) {
            (ScalarKind::Bool, _) => Literal::Bool(value != 0),
            (ScalarKind::Sint, 8) => Literal::I64(i64::from(value)),
            (ScalarKind::Sint, _) => Literal::I32(i32::from(value)),
            (ScalarKind::Uint, 8) => Literal::U64(u64::from(value.cast_unsigned())),
            (ScalarKind::Uint, _) => Literal::U32(u32::from(value.cast_unsigned())),
            (ScalarKind::Float, 2) => {
                Literal::F16(klf_shader::parse::f32_to_half(f32::from(value)))
            }
            (ScalarKind::Float, 8) => Literal::f64(f64::from(value)),
            (ScalarKind::Float, _) => Literal::f32(f32::from(value)),
        };
        let one = self.literal(literal);
        match size {
            Some(size) => {
                let ty = self.vector(size, scalar);
                self.composite_constant(ty, vec![one; usize::from(size)])
            }
            None => one,
        }
    }

    /// A constant of `inner` with `value`.
    pub(crate) fn const_value(
        &mut self,
        inner: &TypeInner,
        name: Option<&str>,
        value: &ConstValue,
    ) -> u32 {
        let module = self.module;
        let ty = self.type_inner(inner, name, false);
        match (value, inner) {
            (ConstValue::Scalar(scalar), _) => self.literal(const_literal(*scalar)),
            (ConstValue::Composite(parts), TypeInner::Vector { scalar, .. }) => {
                let ids = parts
                    .iter()
                    .map(|part| self.const_value(&TypeInner::Scalar(*scalar), None, part))
                    .collect();
                self.composite_constant(ty, ids)
            }
            (ConstValue::Composite(parts), TypeInner::Matrix { rows, scalar, .. }) => {
                let column = TypeInner::Vector {
                    size: *rows,
                    scalar: *scalar,
                };
                let ids = parts
                    .iter()
                    .map(|part| self.const_value(&column, None, part))
                    .collect();
                self.composite_constant(ty, ids)
            }
            (ConstValue::Composite(parts), TypeInner::Array { base, .. }) => {
                let base = &module.types[*base];
                let ids = parts
                    .iter()
                    .map(|part| self.const_value(&base.inner, base.name.as_deref(), part))
                    .collect();
                self.composite_constant(ty, ids)
            }
            (ConstValue::Composite(parts), TypeInner::Struct { members, .. }) => {
                let ids = parts
                    .iter()
                    .zip(members)
                    .map(|(part, member)| {
                        let member = &module.types[member.ty];
                        self.const_value(&member.inner, member.name.as_deref(), part)
                    })
                    .collect();
                self.composite_constant(ty, ids)
            }
            _ => {
                self.refuse("a constant of a type with no constants", Span::default());
                self.null(ty)
            }
        }
    }

    /// The constant a global expression evaluates to, typed `ty`.
    pub(crate) fn global_constant(
        &mut self,
        expression: Handle<Expression>,
        ty: Handle<Type>,
    ) -> u32 {
        let module = self.module;
        let info = self.info;
        let ty = &module.types[ty];
        if let Some(value) = info
            .constants
            .get(expression.index())
            .and_then(Option::as_ref)
        {
            return self.const_value(&ty.inner, ty.name.as_deref(), value);
        }
        self.refuse(
            "a value that depends on a kept override",
            module.global_expressions.span(expression),
        );
        let id = self.type_inner(&ty.inner, ty.name.as_deref(), false);
        self.null(id)
    }
}

/// The value words of a numeric literal; a `bool` has none.
pub(crate) fn literal_words(literal: Literal) -> Vec<u32> {
    match literal {
        Literal::Bool(_) => Vec::new(),
        Literal::I32(value) => vec![value.cast_unsigned()],
        Literal::U32(value) | Literal::F32(value) => vec![value],
        Literal::F16(bits) => vec![u32::from(bits)],
        Literal::I64(value) => split(value.cast_unsigned()),
        Literal::U64(value) | Literal::F64(value) => split(value),
    }
}

fn split(value: u64) -> Vec<u32> {
    vec![value as u32, (value >> 32) as u32]
}

fn const_literal(scalar: ConstScalar) -> Literal {
    scalar.to_literal()
}

/// The `MatrixStride` of a member of type `ty` and `size` bytes, when the
/// member is a matrix or an array of matrices.
fn matrix_stride(module: &klf_shader::Module, ty: Handle<Type>, size: u32) -> Option<u32> {
    match &module.types[ty].inner {
        TypeInner::Matrix { columns, .. } => Some(size / u32::from(*columns)),
        TypeInner::Array { base, stride, .. } => match module.types[*base].inner {
            TypeInner::Matrix { .. } => matrix_stride(module, *base, *stride),
            TypeInner::Array { .. } => matrix_stride(module, *base, 0),
            _ => None,
        },
        _ => None,
    }
}
