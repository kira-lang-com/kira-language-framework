//! Expressions: each evaluated once, where its `emit` places it, into an
//! SSA id.

use crate::function::FunctionWriter;
use crate::words::{class, op};
use klf_shader::resolve::Resolution;
use klf_shader::{ArraySize, Expression, Handle, Span, TypeInner};

impl FunctionWriter<'_, '_> {
    /// The id of an expression's value.
    pub(crate) fn value(&mut self, handle: Handle<Expression>) -> u32 {
        if let Some(id) = self.values.get(handle.index()).copied()
            && id != 0
        {
            return id;
        }
        let function = self.function;
        let id = match &function.expressions[handle] {
            Expression::FunctionArgument(index) => {
                self.arguments.get(*index as usize).copied().unwrap_or(0)
            }
            Expression::LocalVariable(local) => self.locals[local.index()],
            expression if !expression.needs_emit() => self.const_expression(handle).unwrap_or(0),
            _ => 0,
        };
        if id == 0 {
            self.w.refuse(
                "an expression used before it is evaluated",
                function.expressions.span(handle),
            );
            return self.w.literal(klf_shader::Literal::U32(0));
        }
        self.values[handle.index()] = id;
        id
    }

    /// The constant an expression is, when it is one.
    pub(crate) fn const_expression(&mut self, handle: Handle<Expression>) -> Option<u32> {
        let module = self.w.module;
        let function = self.function;
        Some(match &function.expressions[handle] {
            Expression::Literal(literal) => self.w.literal(*literal),
            Expression::Constant(constant) => {
                let constant = &module.constants[*constant];
                self.w.global_constant(constant.init, constant.ty)
            }
            Expression::ZeroValue(ty) => {
                let ty = self.w.type_of(*ty, false);
                self.w.null(ty)
            }
            Expression::Override(item) => self.w.spec_constant(*item),
            Expression::Compose { components, .. } => {
                let parts = components
                    .iter()
                    .map(|part| self.const_expression(*part))
                    .collect::<Option<Vec<u32>>>()?;
                let ty = self.resolved_type(handle);
                self.w.composite_constant(ty, parts)
            }
            Expression::Splat { size, value } => {
                let part = self.const_expression(*value)?;
                let ty = self.resolved_type(handle);
                self.w
                    .composite_constant(ty, vec![part; usize::from(*size)])
            }
            _ => return None,
        })
    }

    pub(crate) fn resolution(&self, handle: Handle<Expression>) -> &Resolution {
        &self.info.expressions[handle.index()].ty
    }

    /// The shape of an expression's type.
    pub(crate) fn inner(&self, handle: Handle<Expression>) -> TypeInner {
        self.resolution(handle).inner(&self.w.module.types).clone()
    }

    /// The plain type id of an expression.
    pub(crate) fn resolved_type(&mut self, handle: Handle<Expression>) -> u32 {
        match self.info.expressions[handle.index()].ty.clone() {
            Resolution::Handle(ty) => self.w.type_of(ty, false),
            Resolution::Value(inner) => self.w.type_inner(&inner, None, false),
        }
    }

    pub(crate) fn expression(&mut self, handle: Handle<Expression>) -> u32 {
        let function = self.function;
        let span = function.expressions.span(handle);
        let expression = &function.expressions[handle];
        match expression {
            Expression::Literal(_)
            | Expression::Constant(_)
            | Expression::Override(_)
            | Expression::ZeroValue(_)
            | Expression::FunctionArgument(_)
            | Expression::GlobalVariable(_)
            | Expression::LocalVariable(_)
            | Expression::CallResult(_)
            | Expression::AtomicResult { .. }
            | Expression::WorkgroupUniformLoadResult { .. } => self.value(handle),
            Expression::Compose { components, .. } => {
                let ty = self.resolved_type(handle);
                let parts: Vec<u32> = components.iter().map(|part| self.value(*part)).collect();
                self.result(op::COMPOSITE_CONSTRUCT, ty, &parts)
            }
            Expression::Splat { size, value } => {
                let ty = self.resolved_type(handle);
                let part = self.value(*value);
                self.result(op::COMPOSITE_CONSTRUCT, ty, &vec![part; usize::from(*size)])
            }
            Expression::Swizzle { vector, pattern } => {
                let ty = self.resolved_type(handle);
                let vector = self.value(*vector);
                let mut operands = vec![vector, vector];
                operands.extend(pattern.iter().map(|component| u32::from(*component)));
                self.result(op::VECTOR_SHUFFLE, ty, &operands)
            }
            Expression::AccessIndex { base, index } => {
                let ty = self.resolved_type(handle);
                let base_id = self.value(*base);
                if matches!(self.inner(*base), TypeInner::Pointer { .. }) {
                    let index = self.w.literal(klf_shader::Literal::U32(*index));
                    self.result(op::ACCESS_CHAIN, ty, &[base_id, index])
                } else {
                    self.result(op::COMPOSITE_EXTRACT, ty, &[base_id, *index])
                }
            }
            Expression::Access { base, index } => {
                let ty = self.resolved_type(handle);
                let base_inner = self.inner(*base);
                let base_id = self.value(*base);
                let index = self.value(*index);
                match base_inner {
                    TypeInner::Pointer { .. } => {
                        self.result(op::ACCESS_CHAIN, ty, &[base_id, index])
                    }
                    TypeInner::Vector { .. } => {
                        self.result(op::VECTOR_EXTRACT_DYNAMIC, ty, &[base_id, index])
                    }
                    _ => {
                        let base_type = self.resolved_type(*base);
                        let variable = self.temporary(base_type);
                        self.emit(op::STORE, &[variable, base_id]);
                        let pointer = self.w.pointer(class::FUNCTION, ty);
                        let element = self.result(op::ACCESS_CHAIN, pointer, &[variable, index]);
                        self.result(op::LOAD, ty, &[element])
                    }
                }
            }
            Expression::Load { pointer } => {
                let source = self.value(*pointer);
                let module = self.w.module;
                let Some((base, space)) = self.pointee(*pointer) else {
                    self.w.refuse("a load through a non-pointer", span);
                    return source;
                };
                if let TypeInner::Atomic(scalar) = module.types[base].inner {
                    let ty = self.w.scalar(scalar);
                    let scope = if space == klf_shader::AddressSpace::Workgroup {
                        2
                    } else {
                        1
                    };
                    let scope = self.w.literal(klf_shader::Literal::U32(scope));
                    let relaxed = self.w.literal(klf_shader::Literal::U32(0));
                    return self.result(op::ATOMIC_LOAD, ty, &[source, scope, relaxed]);
                }
                let layout = space.is_host_shared();
                let ty = self.w.type_of(base, layout);
                let loaded = self.result(op::LOAD, ty, &[source]);
                if layout {
                    self.convert(loaded, base, false)
                } else {
                    loaded
                }
            }
            Expression::ImageSample { .. } => self.image_sample(handle),
            Expression::ImageLoad { .. } => self.image_load(handle),
            Expression::ImageQuery { image, query } => self.image_query(handle, *image, *query),
            Expression::Unary { op, expr } => self.unary(handle, *op, *expr),
            Expression::Binary { op, left, right } => self.binary(handle, *op, *left, *right),
            Expression::Select {
                condition,
                accept,
                reject,
            } => self.select(handle, *condition, *accept, *reject),
            Expression::Derivative {
                axis,
                control,
                expr,
            } => self.derivative(handle, *axis, *control, *expr),
            Expression::Relational { fun, argument } => self.relational(handle, *fun, *argument),
            Expression::Math { fun, args } => self.math(handle, *fun, args),
            Expression::Convert { expr, kind, width } => {
                self.convert_scalar(handle, *expr, *kind, *width)
            }
            Expression::Bitcast { expr, ty } => {
                let ty = self.w.type_of(*ty, false);
                let value = self.value(*expr);
                self.result(op::BITCAST, ty, &[value])
            }
            Expression::ArrayLength(pointer) => self.array_length(*pointer, span),
        }
    }

    fn array_length(&mut self, pointer: Handle<Expression>, span: Span) -> u32 {
        let module = self.w.module;
        let function = self.function;
        let (global, member) = match &function.expressions[pointer] {
            Expression::GlobalVariable(global) => (*global, None),
            Expression::AccessIndex { base, index } => match &function.expressions[*base] {
                Expression::GlobalVariable(global) => (*global, Some(*index)),
                _ => (klf_shader::Handle::new(u32::MAX), None),
            },
            _ => (klf_shader::Handle::new(u32::MAX), None),
        };
        let found = self.w.globals.get(global.index()).copied().flatten();
        let ty = self.w.scalar(klf_shader::Scalar::U32);
        match (found, member) {
            (Some(id), None) if id.wrapped => self.result(op::ARRAY_LENGTH, ty, &[id.variable, 0]),
            (Some(id), Some(member)) if !id.wrapped => {
                let runtime = match &module.types[module.globals[global].ty].inner {
                    TypeInner::Struct { members, .. } => {
                        members.get(member as usize).is_some_and(|field| {
                            matches!(
                                module.types[field.ty].inner,
                                TypeInner::Array {
                                    size: ArraySize::Runtime,
                                    ..
                                }
                            )
                        })
                    }
                    _ => false,
                };
                if !runtime {
                    self.w
                        .refuse("array_length of a member that is not runtime-sized", span);
                }
                self.result(op::ARRAY_LENGTH, ty, &[id.variable, member])
            }
            _ => {
                self.w.refuse(
                    "array_length of a pointer that is not a buffer's tail",
                    span,
                );
                self.w.literal(klf_shader::Literal::U32(0))
            }
        }
    }
}
