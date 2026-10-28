//! Operators, math, relational and derivative functions, and conversions.

use crate::function::FunctionWriter;
use crate::words::{capability, glsl, op};
use klf_shader::{
    BinaryOp, DerivativeAxis, DerivativeControl, Expression, Handle, MathFunction,
    RelationalFunction, Scalar, ScalarKind, Span, TypeInner, UnaryOp,
};

/// The vector size and scalar of a scalar, vector or matrix shape.
fn shape(inner: &TypeInner) -> (Option<u8>, Scalar) {
    match *inner {
        TypeInner::Scalar(scalar) | TypeInner::Atomic(scalar) => (None, scalar),
        TypeInner::Vector { size, scalar } => (Some(size), scalar),
        TypeInner::Matrix { rows, scalar, .. } => (Some(rows), scalar),
        _ => (None, Scalar::U32),
    }
}

impl FunctionWriter<'_, '_> {
    fn span(&self, handle: Handle<Expression>) -> Span {
        self.function.expressions.span(handle)
    }

    /// A scalar operand widened to `size` components.
    fn splat_to(&mut self, value: u32, scalar: Scalar, size: Option<u8>) -> u32 {
        match size {
            Some(size) => {
                let ty = self.w.vector(size, scalar);
                self.result(op::COMPOSITE_CONSTRUCT, ty, &vec![value; usize::from(size)])
            }
            None => value,
        }
    }

    pub(crate) fn unary(
        &mut self,
        handle: Handle<Expression>,
        op_: UnaryOp,
        expr: Handle<Expression>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let (_, scalar) = shape(&self.inner(expr));
        let value = self.value(expr);
        let opcode = match (op_, scalar.kind) {
            (UnaryOp::Negate, ScalarKind::Float) => op::F_NEGATE,
            (UnaryOp::Negate, _) => op::S_NEGATE,
            (UnaryOp::LogicalNot, _) => op::LOGICAL_NOT,
            (UnaryOp::BitwiseNot, _) => op::NOT,
        };
        if let TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } = self.inner(expr)
        {
            let column = self.w.vector(rows, scalar);
            let mut parts = Vec::new();
            for index in 0..u32::from(columns) {
                let part = self.result(op::COMPOSITE_EXTRACT, column, &[value, index]);
                parts.push(self.result(opcode, column, &[part]));
            }
            return self.result(op::COMPOSITE_CONSTRUCT, ty, &parts);
        }
        self.result(opcode, ty, &[value])
    }

    pub(crate) fn binary(
        &mut self,
        handle: Handle<Expression>,
        op_: BinaryOp,
        left: Handle<Expression>,
        right: Handle<Expression>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let left_inner = self.inner(left);
        let right_inner = self.inner(right);
        let mut left_id = self.value(left);
        let mut right_id = self.value(right);
        match (op_, &left_inner, &right_inner) {
            (BinaryOp::Multiply, TypeInner::Matrix { .. }, TypeInner::Matrix { .. }) => {
                return self.result(op::MATRIX_TIMES_MATRIX, ty, &[left_id, right_id]);
            }
            (BinaryOp::Multiply, TypeInner::Matrix { .. }, TypeInner::Vector { .. }) => {
                return self.result(op::MATRIX_TIMES_VECTOR, ty, &[left_id, right_id]);
            }
            (BinaryOp::Multiply, TypeInner::Vector { .. }, TypeInner::Matrix { .. }) => {
                return self.result(op::VECTOR_TIMES_MATRIX, ty, &[left_id, right_id]);
            }
            (BinaryOp::Multiply, TypeInner::Matrix { .. }, TypeInner::Scalar(_)) => {
                return self.result(op::MATRIX_TIMES_SCALAR, ty, &[left_id, right_id]);
            }
            (BinaryOp::Multiply, TypeInner::Scalar(_), TypeInner::Matrix { .. }) => {
                return self.result(op::MATRIX_TIMES_SCALAR, ty, &[right_id, left_id]);
            }
            (BinaryOp::Multiply, TypeInner::Vector { scalar, .. }, TypeInner::Scalar(_))
                if scalar.kind == ScalarKind::Float =>
            {
                return self.result(op::VECTOR_TIMES_SCALAR, ty, &[left_id, right_id]);
            }
            (BinaryOp::Multiply, TypeInner::Scalar(scalar), TypeInner::Vector { .. })
                if scalar.kind == ScalarKind::Float =>
            {
                return self.result(op::VECTOR_TIMES_SCALAR, ty, &[right_id, left_id]);
            }
            (
                BinaryOp::Add | BinaryOp::Subtract,
                TypeInner::Matrix {
                    columns,
                    rows,
                    scalar,
                },
                TypeInner::Matrix { .. },
            ) => {
                let column = self.w.vector(*rows, *scalar);
                let opcode = if op_ == BinaryOp::Add {
                    op::F_ADD
                } else {
                    op::F_SUB
                };
                let mut parts = Vec::new();
                for index in 0..u32::from(*columns) {
                    let a = self.result(op::COMPOSITE_EXTRACT, column, &[left_id, index]);
                    let b = self.result(op::COMPOSITE_EXTRACT, column, &[right_id, index]);
                    parts.push(self.result(opcode, column, &[a, b]));
                }
                return self.result(op::COMPOSITE_CONSTRUCT, ty, &parts);
            }
            _ => {}
        }
        let (left_size, scalar) = shape(&left_inner);
        let (right_size, right_scalar) = shape(&right_inner);
        let shift = matches!(op_, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
        match (left_size, right_size) {
            (Some(size), None) => right_id = self.splat_to(right_id, right_scalar, Some(size)),
            (None, Some(size)) if !shift => left_id = self.splat_to(left_id, scalar, Some(size)),
            _ => {}
        }
        let kind = scalar.kind;
        let opcode = match (op_, kind) {
            (BinaryOp::Add, ScalarKind::Float) => op::F_ADD,
            (BinaryOp::Add, _) => op::I_ADD,
            (BinaryOp::Subtract, ScalarKind::Float) => op::F_SUB,
            (BinaryOp::Subtract, _) => op::I_SUB,
            (BinaryOp::Multiply, ScalarKind::Float) => op::F_MUL,
            (BinaryOp::Multiply, _) => op::I_MUL,
            (BinaryOp::Divide, ScalarKind::Float) => op::F_DIV,
            (BinaryOp::Divide, ScalarKind::Sint) => op::S_DIV,
            (BinaryOp::Divide, _) => op::U_DIV,
            (BinaryOp::Modulo, ScalarKind::Float) => op::F_REM,
            (BinaryOp::Modulo, ScalarKind::Sint) => op::S_REM,
            (BinaryOp::Modulo, _) => op::U_MOD,
            (BinaryOp::Equal, ScalarKind::Bool) => op::LOGICAL_EQUAL,
            (BinaryOp::Equal, ScalarKind::Float) => op::F_ORD_EQUAL,
            (BinaryOp::Equal, _) => op::I_EQUAL,
            (BinaryOp::NotEqual, ScalarKind::Bool) => op::LOGICAL_NOT_EQUAL,
            (BinaryOp::NotEqual, ScalarKind::Float) => op::F_UNORD_NOT_EQUAL,
            (BinaryOp::NotEqual, _) => op::I_NOT_EQUAL,
            (BinaryOp::Less, ScalarKind::Float) => op::F_ORD_LESS_THAN,
            (BinaryOp::Less, ScalarKind::Sint) => op::S_LESS_THAN,
            (BinaryOp::Less, _) => op::U_LESS_THAN,
            (BinaryOp::LessEqual, ScalarKind::Float) => op::F_ORD_LESS_THAN_EQUAL,
            (BinaryOp::LessEqual, ScalarKind::Sint) => op::S_LESS_THAN_EQUAL,
            (BinaryOp::LessEqual, _) => op::U_LESS_THAN_EQUAL,
            (BinaryOp::Greater, ScalarKind::Float) => op::F_ORD_GREATER_THAN,
            (BinaryOp::Greater, ScalarKind::Sint) => op::S_GREATER_THAN,
            (BinaryOp::Greater, _) => op::U_GREATER_THAN,
            (BinaryOp::GreaterEqual, ScalarKind::Float) => op::F_ORD_GREATER_THAN_EQUAL,
            (BinaryOp::GreaterEqual, ScalarKind::Sint) => op::S_GREATER_THAN_EQUAL,
            (BinaryOp::GreaterEqual, _) => op::U_GREATER_THAN_EQUAL,
            (BinaryOp::And, ScalarKind::Bool) | (BinaryOp::LogicalAnd, _) => op::LOGICAL_AND,
            (BinaryOp::And, _) => op::BITWISE_AND,
            (BinaryOp::InclusiveOr, ScalarKind::Bool) | (BinaryOp::LogicalOr, _) => op::LOGICAL_OR,
            (BinaryOp::InclusiveOr, _) => op::BITWISE_OR,
            (BinaryOp::ExclusiveOr, ScalarKind::Bool) => op::LOGICAL_NOT_EQUAL,
            (BinaryOp::ExclusiveOr, _) => op::BITWISE_XOR,
            (BinaryOp::ShiftLeft, _) => op::SHIFT_LEFT_LOGICAL,
            (BinaryOp::ShiftRight, ScalarKind::Sint) => op::SHIFT_RIGHT_ARITHMETIC,
            (BinaryOp::ShiftRight, _) => op::SHIFT_RIGHT_LOGICAL,
        };
        self.result(opcode, ty, &[left_id, right_id])
    }

    pub(crate) fn select(
        &mut self,
        handle: Handle<Expression>,
        condition: Handle<Expression>,
        accept: Handle<Expression>,
        reject: Handle<Expression>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let mut condition_id = self.value(condition);
        let accept_id = self.value(accept);
        let reject_id = self.value(reject);
        match (self.inner(condition), self.inner(accept)) {
            (TypeInner::Scalar(_), TypeInner::Vector { size, .. }) => {
                condition_id = self.splat_to(condition_id, Scalar::BOOL, Some(size));
            }
            (_, TypeInner::Scalar(_) | TypeInner::Vector { .. }) => {}
            _ => self.w.refuse(
                "a select of composite values in SPIR-V 1.3",
                self.span(handle),
            ),
        }
        self.result(op::SELECT, ty, &[condition_id, accept_id, reject_id])
    }

    pub(crate) fn derivative(
        &mut self,
        handle: Handle<Expression>,
        axis: DerivativeAxis,
        control: DerivativeControl,
        expr: Handle<Expression>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let value = self.value(expr);
        if control != DerivativeControl::None {
            self.w.capabilities.insert(capability::DERIVATIVE_CONTROL);
        }
        let opcode = match (axis, control) {
            (DerivativeAxis::X, DerivativeControl::None) => op::DPDX,
            (DerivativeAxis::X, DerivativeControl::Fine) => op::DPDX_FINE,
            (DerivativeAxis::X, DerivativeControl::Coarse) => op::DPDX_COARSE,
            (DerivativeAxis::Y, DerivativeControl::None) => op::DPDY,
            (DerivativeAxis::Y, DerivativeControl::Fine) => op::DPDY_FINE,
            (DerivativeAxis::Y, DerivativeControl::Coarse) => op::DPDY_COARSE,
            (DerivativeAxis::Width, DerivativeControl::None) => op::FWIDTH,
            (DerivativeAxis::Width, DerivativeControl::Fine) => op::FWIDTH_FINE,
            (DerivativeAxis::Width, DerivativeControl::Coarse) => op::FWIDTH_COARSE,
        };
        self.result(opcode, ty, &[value])
    }

    pub(crate) fn relational(
        &mut self,
        handle: Handle<Expression>,
        fun: RelationalFunction,
        argument: Handle<Expression>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let value = self.value(argument);
        let scalar_argument = matches!(self.inner(argument), TypeInner::Scalar(_));
        let opcode = match fun {
            RelationalFunction::All | RelationalFunction::Any if scalar_argument => return value,
            RelationalFunction::All => op::ALL,
            RelationalFunction::Any => op::ANY,
            RelationalFunction::IsNan => op::IS_NAN,
            RelationalFunction::IsInf => op::IS_INF,
        };
        self.result(opcode, ty, &[value])
    }

    fn ext(&mut self, ty: u32, instruction: u32, arguments: &[u32]) -> u32 {
        let mut operands = vec![self.w.glsl, instruction];
        operands.extend_from_slice(arguments);
        self.result(op::EXT_INST, ty, &operands)
    }

    pub(crate) fn math(
        &mut self,
        handle: Handle<Expression>,
        fun: MathFunction,
        args: &[Handle<Expression>],
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let first = self.inner(args[0]);
        let (size, scalar) = shape(&first);
        let kind = scalar.kind;
        let widen = matches!(
            fun,
            MathFunction::Min
                | MathFunction::Max
                | MathFunction::Clamp
                | MathFunction::Mix
                | MathFunction::Step
                | MathFunction::SmoothStep
                | MathFunction::Fma
                | MathFunction::Atan2
                | MathFunction::Pow
        );
        let result_size = shape(&self.inner(handle)).0;
        let mut ids = Vec::new();
        for argument in args {
            let id = self.value(*argument);
            let (argument_size, argument_scalar) = shape(&self.inner(*argument));
            ids.push(match (widen, argument_size, result_size) {
                (true, None, Some(_)) => self.splat_to(id, argument_scalar, result_size),
                _ => id,
            });
        }
        let pick = |float: u32, sint: u32, uint: u32| match kind {
            ScalarKind::Float => float,
            ScalarKind::Sint => sint,
            _ => uint,
        };
        let instruction = match fun {
            MathFunction::Abs if kind == ScalarKind::Uint => return ids[0],
            MathFunction::Abs => pick(glsl::F_ABS, glsl::S_ABS, glsl::S_ABS),
            MathFunction::Min => pick(glsl::F_MIN, glsl::S_MIN, glsl::U_MIN),
            MathFunction::Max => pick(glsl::F_MAX, glsl::S_MAX, glsl::U_MAX),
            MathFunction::Clamp => pick(glsl::F_CLAMP, glsl::S_CLAMP, glsl::U_CLAMP),
            MathFunction::Saturate => {
                let zero = self.w.number(scalar, 0, size);
                let one = self.w.number(scalar, 1, size);
                return self.ext(ty, glsl::F_CLAMP, &[ids[0], zero, one]);
            }
            MathFunction::Sign => pick(glsl::F_SIGN, glsl::S_SIGN, glsl::S_SIGN),
            MathFunction::Sin => glsl::SIN,
            MathFunction::Cos => glsl::COS,
            MathFunction::Tan => glsl::TAN,
            MathFunction::Asin => glsl::ASIN,
            MathFunction::Acos => glsl::ACOS,
            MathFunction::Atan => glsl::ATAN,
            MathFunction::Atan2 => glsl::ATAN2,
            MathFunction::Sinh => glsl::SINH,
            MathFunction::Cosh => glsl::COSH,
            MathFunction::Tanh => glsl::TANH,
            MathFunction::Asinh => glsl::ASINH,
            MathFunction::Acosh => glsl::ACOSH,
            MathFunction::Atanh => glsl::ATANH,
            MathFunction::Radians => glsl::RADIANS,
            MathFunction::Degrees => glsl::DEGREES,
            MathFunction::Ceil => glsl::CEIL,
            MathFunction::Floor => glsl::FLOOR,
            MathFunction::Round => glsl::ROUND_EVEN,
            MathFunction::Fract => glsl::FRACT,
            MathFunction::Trunc => glsl::TRUNC,
            MathFunction::Exp => glsl::EXP,
            MathFunction::Exp2 => glsl::EXP2,
            MathFunction::Log => glsl::LOG,
            MathFunction::Log2 => glsl::LOG2,
            MathFunction::Pow => glsl::POW,
            MathFunction::Sqrt => glsl::SQRT,
            MathFunction::InverseSqrt => glsl::INVERSE_SQRT,
            MathFunction::Dot if kind == ScalarKind::Float => {
                return self.result(op::DOT, ty, &ids);
            }
            MathFunction::Dot => {
                let vector = self.resolved_type(args[0]);
                let product = self.result(op::I_MUL, vector, &ids);
                let mut sum = self.result(op::COMPOSITE_EXTRACT, ty, &[product, 0]);
                for index in 1..u32::from(size.unwrap_or(1)) {
                    let part = self.result(op::COMPOSITE_EXTRACT, ty, &[product, index]);
                    sum = self.result(op::I_ADD, ty, &[sum, part]);
                }
                return sum;
            }
            MathFunction::Cross => glsl::CROSS,
            MathFunction::Distance => glsl::DISTANCE,
            MathFunction::Length => glsl::LENGTH,
            MathFunction::Normalize => glsl::NORMALIZE,
            MathFunction::FaceForward => glsl::FACE_FORWARD,
            MathFunction::Reflect => glsl::REFLECT,
            MathFunction::Refract => glsl::REFRACT,
            MathFunction::Mix => glsl::F_MIX,
            MathFunction::Step => glsl::STEP,
            MathFunction::SmoothStep => glsl::SMOOTH_STEP,
            MathFunction::Fma => glsl::FMA,
            MathFunction::Transpose => return self.result(op::TRANSPOSE, ty, &ids),
            MathFunction::Determinant => glsl::DETERMINANT,
            MathFunction::CountOneBits => return self.result(op::BIT_COUNT, ty, &ids),
            MathFunction::ReverseBits => return self.result(op::BIT_REVERSE, ty, &ids),
            MathFunction::FirstLeadingBit => {
                pick(glsl::FIND_U_MSB, glsl::FIND_S_MSB, glsl::FIND_U_MSB)
            }
            MathFunction::FirstTrailingBit => glsl::FIND_I_LSB,
            MathFunction::CountLeadingZeros | MathFunction::CountTrailingZeros => {
                return self.count_zeros(fun, ty, ids[0], size, scalar);
            }
            MathFunction::ExtractBits => {
                let opcode = if kind == ScalarKind::Sint {
                    op::BIT_FIELD_S_EXTRACT
                } else {
                    op::BIT_FIELD_U_EXTRACT
                };
                return self.result(opcode, ty, &ids);
            }
            MathFunction::InsertBits => return self.result(op::BIT_FIELD_INSERT, ty, &ids),
            MathFunction::Pack4x8Snorm => glsl::PACK_SNORM_4X8,
            MathFunction::Pack4x8Unorm => glsl::PACK_UNORM_4X8,
            MathFunction::Pack2x16Snorm => glsl::PACK_SNORM_2X16,
            MathFunction::Pack2x16Unorm => glsl::PACK_UNORM_2X16,
            MathFunction::Pack2x16Float => glsl::PACK_HALF_2X16,
            MathFunction::Unpack4x8Snorm => glsl::UNPACK_SNORM_4X8,
            MathFunction::Unpack4x8Unorm => glsl::UNPACK_UNORM_4X8,
            MathFunction::Unpack2x16Snorm => glsl::UNPACK_SNORM_2X16,
            MathFunction::Unpack2x16Unorm => glsl::UNPACK_UNORM_2X16,
            MathFunction::Unpack2x16Float => glsl::UNPACK_HALF_2X16,
        };
        self.ext(ty, instruction, &ids)
    }

    /// Leading zeros are `31 - msb`, trailing zeros `min(lsb, 32)`,
    /// computed unsigned so that zero gives 32.
    fn count_zeros(
        &mut self,
        fun: MathFunction,
        ty: u32,
        value: u32,
        size: Option<u8>,
        scalar: Scalar,
    ) -> u32 {
        let unsigned = Scalar {
            kind: ScalarKind::Uint,
            width: scalar.width,
        };
        let uint = self.w.scalar_or_vector(size, unsigned);
        let value = if scalar.kind == ScalarKind::Sint {
            self.result(op::BITCAST, uint, &[value])
        } else {
            value
        };
        let counted = if fun == MathFunction::CountLeadingZeros {
            let msb = self.ext(uint, glsl::FIND_U_MSB, &[value]);
            let top = self.w.number(unsigned, 31, size);
            self.result(op::I_SUB, uint, &[top, msb])
        } else {
            let lsb = self.ext(uint, glsl::FIND_I_LSB, &[value]);
            let limit = self.w.number(unsigned, 32, size);
            self.ext(uint, glsl::U_MIN, &[lsb, limit])
        };
        if scalar.kind == ScalarKind::Sint {
            self.result(op::BITCAST, ty, &[counted])
        } else {
            counted
        }
    }

    pub(crate) fn convert_scalar(
        &mut self,
        handle: Handle<Expression>,
        expr: Handle<Expression>,
        kind: ScalarKind,
        width: Option<u8>,
    ) -> u32 {
        let ty = self.resolved_type(handle);
        let value = self.value(expr);
        let (size, source) = shape(&self.inner(expr));
        let target = Scalar {
            kind,
            width: if kind == ScalarKind::Bool {
                1
            } else {
                width.unwrap_or(source.width)
            },
        };
        if source == target {
            return value;
        }
        match (source.kind, kind) {
            (ScalarKind::Bool, _) => {
                let one = self.w.number(target, 1, size);
                let zero = self.w.number(target, 0, size);
                self.result(op::SELECT, ty, &[value, one, zero])
            }
            (from, ScalarKind::Bool) => {
                let zero = self.w.number(source, 0, size);
                let opcode = if from == ScalarKind::Float {
                    op::F_UNORD_NOT_EQUAL
                } else {
                    op::I_NOT_EQUAL
                };
                self.result(opcode, ty, &[value, zero])
            }
            (ScalarKind::Float, ScalarKind::Float) => self.result(op::F_CONVERT, ty, &[value]),
            (ScalarKind::Float, ScalarKind::Sint) => self.result(op::CONVERT_F_TO_S, ty, &[value]),
            (ScalarKind::Float, _) => self.result(op::CONVERT_F_TO_U, ty, &[value]),
            (ScalarKind::Sint, ScalarKind::Float) => self.result(op::CONVERT_S_TO_F, ty, &[value]),
            (_, ScalarKind::Float) => self.result(op::CONVERT_U_TO_F, ty, &[value]),
            (from, _) => {
                let resized = if source.width == target.width {
                    value
                } else {
                    let wide = Scalar {
                        kind: from,
                        width: target.width,
                    };
                    let wide = self.w.scalar_or_vector(size, wide);
                    let opcode = if from == ScalarKind::Sint {
                        op::S_CONVERT
                    } else {
                        op::U_CONVERT
                    };
                    self.result(opcode, wide, &[value])
                };
                if from == kind {
                    resized
                } else {
                    self.result(op::BITCAST, ty, &[resized])
                }
            }
        }
    }
}
