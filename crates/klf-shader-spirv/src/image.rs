//! Sampling, loads, stores and queries (section 19.4 rule 6).

use crate::function::FunctionWriter;
use crate::words::{capability, op};
use klf_shader::{
    Expression, Handle, ImageClass, ImageDim, ImageQuery, Literal, SampleLevel, Scalar, ScalarKind,
    TypeInner,
};

const BIAS: u32 = 0x1;
const LOD: u32 = 0x2;
const GRAD: u32 = 0x4;
const CONST_OFFSET: u32 = 0x8;
const OFFSET: u32 = 0x10;
const SAMPLE: u32 = 0x40;

fn image_shape(inner: &TypeInner) -> (ImageDim, bool, ImageClass) {
    match *inner {
        TypeInner::Image {
            dim,
            arrayed,
            class,
        } => (dim, arrayed, class),
        _ => (
            ImageDim::D2,
            false,
            ImageClass::Sampled {
                kind: ScalarKind::Float,
                multisampled: false,
            },
        ),
    }
}

impl FunctionWriter<'_, '_> {
    /// The coordinate with the array index appended as its last component,
    /// converted to the coordinate's scalar.
    fn arrayed_coordinate(
        &mut self,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
    ) -> u32 {
        let value = self.value(coordinate);
        let Some(index) = array_index else {
            return value;
        };
        let (size, scalar) = match self.inner(coordinate) {
            TypeInner::Vector { size, scalar } => (size, scalar),
            TypeInner::Scalar(scalar) => (1, scalar),
            _ => (1, Scalar::F32),
        };
        let index_scalar = self.inner(index).scalar().unwrap_or(Scalar::U32);
        let mut index_id = self.value(index);
        if index_scalar.kind != scalar.kind {
            let ty = self.w.scalar(scalar);
            let opcode = match (index_scalar.kind, scalar.kind) {
                (ScalarKind::Sint, ScalarKind::Float) => op::CONVERT_S_TO_F,
                (_, ScalarKind::Float) => op::CONVERT_U_TO_F,
                _ => op::BITCAST,
            };
            index_id = self.result(opcode, ty, &[index_id]);
        }
        let ty = self.w.vector(size + 1, scalar);
        self.result(op::COMPOSITE_CONSTRUCT, ty, &[value, index_id])
    }

    pub(crate) fn image_sample(&mut self, handle: Handle<Expression>) -> u32 {
        let Expression::ImageSample {
            image,
            sampler,
            gather,
            coordinate,
            array_index,
            offset,
            level,
            depth_ref,
        } = self.function.expressions[handle].clone()
        else {
            return 0;
        };
        let ty = self.resolved_type(handle);
        let (_, _, class) = image_shape(&self.inner(image));
        let image_type = self.resolved_type(image);
        let image_id = self.value(image);
        let sampler_id = self.value(sampler);
        let sampled_type = self.w.sampled_image(image_type);
        let combined = self.result(op::SAMPLED_IMAGE, sampled_type, &[image_id, sampler_id]);
        let coordinate = self.arrayed_coordinate(coordinate, array_index);
        let mut mask = 0;
        let mut extra = Vec::new();
        match level {
            SampleLevel::Auto => {}
            SampleLevel::Zero => {
                mask |= LOD;
                extra.push(self.w.literal(Literal::f32(0.0)));
            }
            SampleLevel::Exact(value) => {
                mask |= LOD;
                extra.push(self.value(value));
            }
            SampleLevel::Bias(value) => {
                mask |= BIAS;
                extra.push(self.value(value));
            }
            SampleLevel::Gradient { x, y } => {
                mask |= GRAD;
                extra.push(self.value(x));
                extra.push(self.value(y));
            }
        }
        if let Some(offset) = offset {
            if let Some(constant) = self.const_expression(offset) {
                mask |= CONST_OFFSET;
                extra.push(constant);
            } else {
                mask |= OFFSET;
                self.w
                    .capabilities
                    .insert(capability::IMAGE_GATHER_EXTENDED);
                extra.push(self.value(offset));
            }
        }
        let mut operands = vec![combined, coordinate];
        let vec4 = |writer: &mut Self, kind| writer.w.vector(4, Scalar { kind, width: 4 });
        let (opcode, result_type) = match (gather, depth_ref) {
            (Some(_), Some(reference)) => {
                operands.push(self.value(reference));
                (op::IMAGE_DREF_GATHER, vec4(self, ScalarKind::Float))
            }
            (Some(component), None) => {
                operands.push(self.w.literal(Literal::U32(u32::from(component))));
                let kind = match class {
                    ImageClass::Sampled { kind, .. } => kind,
                    _ => ScalarKind::Float,
                };
                (op::IMAGE_GATHER, vec4(self, kind))
            }
            (None, Some(reference)) => {
                operands.push(self.value(reference));
                let opcode = if mask & LOD != 0 || mask & GRAD != 0 {
                    op::IMAGE_SAMPLE_DREF_EXPLICIT_LOD
                } else {
                    op::IMAGE_SAMPLE_DREF_IMPLICIT_LOD
                };
                (opcode, ty)
            }
            (None, None) => {
                let opcode = if mask & LOD != 0 || mask & GRAD != 0 {
                    op::IMAGE_SAMPLE_EXPLICIT_LOD
                } else {
                    op::IMAGE_SAMPLE_IMPLICIT_LOD
                };
                let kind = match class {
                    ImageClass::Sampled { kind, .. } => kind,
                    _ => ScalarKind::Float,
                };
                (opcode, vec4(self, kind))
            }
        };
        if mask != 0 {
            operands.push(mask);
            operands.extend(extra);
        }
        let sampled = self.result(opcode, result_type, &operands);
        if result_type == ty {
            sampled
        } else {
            self.result(op::COMPOSITE_EXTRACT, ty, &[sampled, 0])
        }
    }

    pub(crate) fn image_load(&mut self, handle: Handle<Expression>) -> u32 {
        let Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            sample,
            level,
        } = self.function.expressions[handle].clone()
        else {
            return 0;
        };
        let ty = self.resolved_type(handle);
        let (_, _, class) = image_shape(&self.inner(image));
        let image_id = self.value(image);
        let coordinate = self.arrayed_coordinate(coordinate, array_index);
        let (kind, storage) = match class {
            ImageClass::Sampled { kind, .. } => (kind, false),
            ImageClass::Depth { .. } => (ScalarKind::Float, false),
            ImageClass::Storage { format, .. } => (format.kind(), true),
        };
        let result_type = self.w.vector(4, Scalar { kind, width: 4 });
        let loaded = if storage {
            self.result(op::IMAGE_READ, result_type, &[image_id, coordinate])
        } else {
            let mut operands = vec![image_id, coordinate];
            match (sample, level) {
                (Some(sample), _) => {
                    operands.push(SAMPLE);
                    operands.push(self.value(sample));
                }
                (None, Some(level)) => {
                    operands.push(LOD);
                    operands.push(self.value(level));
                }
                (None, None) => {
                    operands.push(LOD);
                    operands.push(self.w.literal(Literal::I32(0)));
                }
            }
            self.result(op::IMAGE_FETCH, result_type, &operands)
        };
        if result_type == ty {
            loaded
        } else {
            self.result(op::COMPOSITE_EXTRACT, ty, &[loaded, 0])
        }
    }

    pub(crate) fn image_store(
        &mut self,
        image: Handle<Expression>,
        coordinate: Handle<Expression>,
        array_index: Option<Handle<Expression>>,
        value: Handle<Expression>,
    ) {
        let image = self.value(image);
        let coordinate = self.arrayed_coordinate(coordinate, array_index);
        let value = self.value(value);
        self.emit(op::IMAGE_WRITE, &[image, coordinate, value]);
    }

    pub(crate) fn image_query(
        &mut self,
        handle: Handle<Expression>,
        image: Handle<Expression>,
        query: ImageQuery,
    ) -> u32 {
        self.w.capabilities.insert(capability::IMAGE_QUERY);
        let ty = self.resolved_type(handle);
        let (dim, arrayed, class) = image_shape(&self.inner(image));
        let image_id = self.value(image);
        let multisampled = match class {
            ImageClass::Sampled { multisampled, .. } | ImageClass::Depth { multisampled } => {
                multisampled
            }
            ImageClass::Storage { .. } => true,
        };
        let uint = Scalar::U32;
        let size = |writer: &mut Self, level: Option<Handle<Expression>>| {
            let components = match dim {
                ImageDim::D1 => 1,
                ImageDim::D2 | ImageDim::Cube => 2,
                ImageDim::D3 => 3,
            } + u8::from(arrayed);
            let size_type = writer
                .w
                .scalar_or_vector((components > 1).then_some(components), uint);
            let queried = if multisampled {
                writer.result(op::IMAGE_QUERY_SIZE, size_type, &[image_id])
            } else {
                let level = match level {
                    Some(level) => writer.value(level),
                    None => writer.w.literal(Literal::U32(0)),
                };
                writer.result(op::IMAGE_QUERY_SIZE_LOD, size_type, &[image_id, level])
            };
            (queried, components)
        };
        match query {
            ImageQuery::Size { level } => {
                let (queried, components) = size(self, level);
                if !arrayed {
                    return queried;
                }
                let wanted = components - 1;
                if wanted == 1 {
                    self.result(op::COMPOSITE_EXTRACT, ty, &[queried, 0])
                } else {
                    let pattern: Vec<u32> = (0..u32::from(wanted)).collect();
                    let mut operands = vec![queried, queried];
                    operands.extend(pattern);
                    self.result(op::VECTOR_SHUFFLE, ty, &operands)
                }
            }
            ImageQuery::NumLayers => {
                let (queried, components) = size(self, None);
                self.result(
                    op::COMPOSITE_EXTRACT,
                    ty,
                    &[queried, u32::from(components - 1)],
                )
            }
            ImageQuery::NumLevels => self.result(op::IMAGE_QUERY_LEVELS, ty, &[image_id]),
            ImageQuery::NumSamples => self.result(op::IMAGE_QUERY_SAMPLES, ty, &[image_id]),
        }
    }
}
