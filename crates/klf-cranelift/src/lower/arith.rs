//! Constants, arithmetic, comparisons and conversions for the Cranelift backend.

use super::FnLowerer;
use crate::{EmitError, err};
use cranelift::codegen::ir::MemFlagsData;
use cranelift::prelude::{FloatCC, InstBuilder, IntCC, Value, types};
use cranelift_module::Module;
use klf_core::Name;
use klf_kir::{
    BinaryOperator, ConvertKind, FloatBinaryOperator, FloatPredicate, FloatUnaryOperator,
    IntegerPredicate, Overflow, Type,
};

pub(crate) fn scalar_bits(ty: &Type) -> Result<u32, EmitError> {
    Ok(match ty {
        Type::Integer(bits) | Type::Float(bits) => u32::from(*bits),
        Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_)
        | Type::Function(_) => 64,
        other => return Err(err(format!("cannot size `{other}`"))),
    })
}

fn signed_bounds(bits: u32) -> (i128, i128) {
    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
}

fn unsigned_max(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

/// A KIR constant is `i128`, but it only ever materializes into a fixed-width
/// Cranelift integer, where `iconst` takes an `i64` and masks to the value's
/// width. The low 64 bits are exactly the intended pattern, so the truncation
/// is deliberate.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn imm64(value: i128) -> i64 {
    value as i64
}

fn wide_type(bits: u32) -> types::Type {
    match bits {
        8 => types::I16,
        16 => types::I32,
        32 => types::I64,
        _ => types::I128,
    }
}

impl FnLowerer<'_> {
    pub(super) fn const_string(&mut self, results: &[Name], value: &str) -> Result<(), EmitError> {
        let data_id = *self
            .strings
            .get(value)
            .ok_or_else(|| err(format!("missing const.string data for `{value}`")))?;
        let data = self.module.declare_data_in_func(data_id, self.builder.func);
        let address = self.builder.ins().symbol_value(types::I64, data);
        let length = self
            .builder
            .ins()
            .iconst(types::I64, i64::try_from(value.len()).unwrap_or(i64::MAX));
        self.define_pair(results, address, length)
    }

    pub(super) fn const_int(
        &mut self,
        results: &[Name],
        ty: &Type,
        value: i128,
    ) -> Result<(), EmitError> {
        let cl = super::cl_type(ty)?;
        let v = self.builder.ins().iconst(cl, imm64(value));
        self.define(results, v)
    }

    pub(super) fn const_zero(&mut self, results: &[Name], ty: &Type) -> Result<(), EmitError> {
        match ty {
            Type::Float(32) => {
                let v = self.builder.ins().f32const(0.0);
                self.define(results, v)
            }
            Type::Float(64) => {
                let v = self.builder.ins().f64const(0.0);
                self.define(results, v)
            }
            _ => self.const_int(results, ty, 0),
        }
    }

    pub(super) fn const_float(
        &mut self,
        results: &[Name],
        ty: &Type,
        literal: &str,
    ) -> Result<(), EmitError> {
        let value = match ty {
            Type::Float(32) => {
                let parsed: f32 = literal
                    .parse()
                    .map_err(|_| err(format!("bad float literal `{literal}`")))?;
                self.builder.ins().f32const(parsed)
            }
            Type::Float(64) => {
                let parsed: f64 = literal
                    .parse()
                    .map_err(|_| err(format!("bad float literal `{literal}`")))?;
                self.builder.ins().f64const(parsed)
            }
            other => return Err(err(format!("unsupported float type `{other}`"))),
        };
        self.define(results, value)
    }

    pub(super) fn binary(
        &mut self,
        results: &[Name],
        operator: BinaryOperator,
        overflow: Overflow,
        signed: bool,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        use BinaryOperator as B;
        let ty = self.ty_of(lhs)?.clone();
        let cl = super::cl_type(&ty)?;
        let bits = scalar_bits(&ty)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let value = match operator {
            B::Add | B::Sub | B::Mul => match overflow {
                Overflow::Wrap => match operator {
                    B::Add => self.builder.ins().iadd(a, b),
                    B::Sub => self.builder.ins().isub(a, b),
                    _ => self.builder.ins().imul(a, b),
                },
                Overflow::Trap => self.trap_arith(operator, signed, cl, bits, a, b),
                Overflow::Saturate => self.saturate_arith(operator, signed, cl, bits, a, b),
            },
            B::And => self.builder.ins().band(a, b),
            B::Or => self.builder.ins().bor(a, b),
            B::Xor => self.builder.ins().bxor(a, b),
            B::UnsignedDiv | B::UnsignedRem | B::SignedDiv | B::SignedRem => {
                self.guarded_div(operator, cl, bits, a, b, overflow)
            }
            B::ShiftLeft | B::LogicalShiftRight | B::ArithmeticShiftRight => {
                self.shift(operator, cl, bits, a, b, overflow)
            }
            B::RotateLeft => self.builder.ins().rotl(a, b),
            B::RotateRight => self.builder.ins().rotr(a, b),
        };
        self.define(results, value)
    }

    fn widen(&mut self, signed: bool, wide: types::Type, value: Value) -> Value {
        if signed {
            self.builder.ins().sextend(wide, value)
        } else {
            self.builder.ins().uextend(wide, value)
        }
    }

    fn checked_bounds(&mut self, signed: bool, wide: types::Type, bits: u32) -> (Value, Value) {
        let (min, max) = if signed {
            signed_bounds(bits)
        } else {
            (
                0,
                i128::try_from(unsigned_max(bits)).expect("width <= 64 fits i128"),
            )
        };
        (
            self.wide_const(signed, wide, min),
            self.wide_const(signed, wide, max),
        )
    }

    /// `add.ovf`, `sub.ovf`, `mul.ovf`: the wrapped result and whether the
    /// exact one left the width.
    pub(super) fn binary_overflow(
        &mut self,
        results: &[Name],
        operator: BinaryOperator,
        signed: bool,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let ty = self.ty_of(lhs)?.clone();
        let cl = super::cl_type(&ty)?;
        let bits = scalar_bits(&ty)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let (result, overflowed) = self.overflowing(operator, signed, cl, bits, a, b);
        self.define_pair(results, result, overflowed)
    }

    fn trap_arith(
        &mut self,
        operator: BinaryOperator,
        signed: bool,
        cl: types::Type,
        bits: u32,
        a: Value,
        b: Value,
    ) -> Value {
        let (result, overflowed) = self.overflowing(operator, signed, cl, bits, a, b);
        self.guard_trap(overflowed, 2);
        result
    }

    fn overflowing(
        &mut self,
        operator: BinaryOperator,
        signed: bool,
        cl: types::Type,
        bits: u32,
        a: Value,
        b: Value,
    ) -> (Value, Value) {
        // At a machine width the overflow flag answers directly; a narrower
        // width checks the wider result against its bounds.
        if bits == cl.bits() && matches!(cl, types::I32 | types::I64) {
            let ins = self.builder.ins();
            return match (operator, signed) {
                (BinaryOperator::Add, true) => ins.sadd_overflow(a, b),
                (BinaryOperator::Add, false) => ins.uadd_overflow(a, b),
                (BinaryOperator::Sub, true) => ins.ssub_overflow(a, b),
                (BinaryOperator::Sub, false) => ins.usub_overflow(a, b),
                (_, true) => ins.smul_overflow(a, b),
                (_, false) => ins.umul_overflow(a, b),
            };
        }
        let wide = wide_type(bits);
        let wa = self.widen(signed, wide, a);
        let wb = self.widen(signed, wide, b);
        let full = match operator {
            BinaryOperator::Add => self.builder.ins().iadd(wa, wb),
            BinaryOperator::Sub => self.builder.ins().isub(wa, wb),
            _ => self.builder.ins().imul(wa, wb),
        };
        let (min_c, max_c) = self.checked_bounds(signed, wide, bits);
        let (lt, gt) = if signed {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let lo = self.builder.ins().icmp(lt, full, min_c);
        let hi = self.builder.ins().icmp(gt, full, max_c);
        let bad = self.builder.ins().bor(lo, hi);
        (self.builder.ins().ireduce(cl, full), bad)
    }

    fn saturate_arith(
        &mut self,
        operator: BinaryOperator,
        signed: bool,
        cl: types::Type,
        bits: u32,
        a: Value,
        b: Value,
    ) -> Value {
        // Cranelift's `*_sat` instructions are vector-only, so saturating scalar
        // arithmetic widens, operates, and clamps to the value width's bounds.
        let wide = wide_type(bits);
        let wa = self.widen(signed, wide, a);
        let wb = self.widen(signed, wide, b);
        let full = match operator {
            BinaryOperator::Add => self.builder.ins().iadd(wa, wb),
            BinaryOperator::Sub => self.builder.ins().isub(wa, wb),
            _ => self.builder.ins().imul(wa, wb),
        };
        let (min_c, max_c) = self.checked_bounds(signed, wide, bits);
        let (lt, gt) = if signed {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let lo = self.builder.ins().icmp(lt, full, min_c);
        let clamped_low = self.builder.ins().select(lo, min_c, full);
        let hi = self.builder.ins().icmp(gt, clamped_low, max_c);
        let clamped = self.builder.ins().select(hi, max_c, clamped_low);
        self.builder.ins().ireduce(cl, clamped)
    }

    fn wide_const(&mut self, signed: bool, wide: types::Type, value: i128) -> Value {
        if wide == types::I128 {
            let low = self.builder.ins().iconst(types::I64, imm64(value));
            self.widen(signed, types::I128, low)
        } else {
            self.builder.ins().iconst(wide, imm64(value))
        }
    }

    fn guarded_div(
        &mut self,
        operator: BinaryOperator,
        cl: types::Type,
        bits: u32,
        a: Value,
        b: Value,
        overflow: Overflow,
    ) -> Value {
        use BinaryOperator as B;
        let signed = matches!(operator, B::SignedDiv | B::SignedRem);
        let is_div = matches!(operator, B::SignedDiv | B::UnsignedDiv);
        let zero = self.builder.ins().iconst(cl, 0);
        let by_zero = self.builder.ins().icmp(IntCC::Equal, b, zero);
        self.guard_trap(by_zero, 10);
        if !signed {
            return if is_div {
                self.builder.ins().udiv(a, b)
            } else {
                self.builder.ins().urem(a, b)
            };
        }
        let (min, _) = signed_bounds(bits);
        let min_c = self.builder.ins().iconst(cl, imm64(min));
        let neg1 = self.builder.ins().iconst(cl, -1);
        let is_min = self.builder.ins().icmp(IntCC::Equal, a, min_c);
        let is_neg1 = self.builder.ins().icmp(IntCC::Equal, b, neg1);
        let overflowing = self.builder.ins().band(is_min, is_neg1);
        if matches!(overflow, Overflow::Trap) {
            self.guard_trap(overflowing, 2);
            return if is_div {
                self.builder.ins().sdiv(a, b)
            } else {
                self.builder.ins().srem(a, b)
            };
        }
        let special = self.builder.create_block();
        let normal = self.builder.create_block();
        let merge = self.builder.create_block();
        self.builder.append_block_param(merge, cl);
        self.builder
            .ins()
            .brif(overflowing, special, &[], normal, &[]);
        self.builder.switch_to_block(special);
        let special_value = if is_div {
            self.builder.ins().iconst(cl, imm64(min))
        } else {
            self.builder.ins().iconst(cl, 0)
        };
        self.builder.ins().jump(merge, &[special_value.into()]);
        self.builder.switch_to_block(normal);
        let normal_value = if is_div {
            self.builder.ins().sdiv(a, b)
        } else {
            self.builder.ins().srem(a, b)
        };
        self.builder.ins().jump(merge, &[normal_value.into()]);
        self.builder.switch_to_block(merge);
        self.builder.block_params(merge)[0]
    }

    fn shift(
        &mut self,
        operator: BinaryOperator,
        cl: types::Type,
        bits: u32,
        a: Value,
        b: Value,
        overflow: Overflow,
    ) -> Value {
        use BinaryOperator as B;
        if matches!(overflow, Overflow::Trap) {
            let limit = self.builder.ins().iconst(cl, i64::from(bits));
            let oob = self
                .builder
                .ins()
                .icmp(IntCC::UnsignedGreaterThanOrEqual, b, limit);
            self.guard_trap(oob, 2);
        }
        match operator {
            B::ShiftLeft => self.builder.ins().ishl(a, b),
            B::LogicalShiftRight => self.builder.ins().ushr(a, b),
            _ => self.builder.ins().sshr(a, b),
        }
    }

    fn guard_trap(&mut self, condition: Value, code: i32) {
        let trap_block = self.builder.create_block();
        self.builder.set_cold_block(trap_block);
        let cont = self.builder.create_block();
        self.builder
            .ins()
            .brif(condition, trap_block, &[], cont, &[]);
        self.builder.switch_to_block(trap_block);
        self.trap(code);
        self.builder.switch_to_block(cont);
    }

    pub(super) fn icmp(
        &mut self,
        results: &[Name],
        predicate: IntegerPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let value = self.builder.ins().icmp(int_cc(predicate), a, b);
        self.define(results, value)
    }

    pub(super) fn fcmp(
        &mut self,
        results: &[Name],
        predicate: FloatPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let value = self.builder.ins().fcmp(float_cc(predicate), a, b);
        self.define(results, value)
    }

    pub(super) fn float_binary(
        &mut self,
        results: &[Name],
        operator: FloatBinaryOperator,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        use FloatBinaryOperator as F;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let value = match operator {
            F::Add => self.builder.ins().fadd(a, b),
            F::Sub => self.builder.ins().fsub(a, b),
            F::Mul => self.builder.ins().fmul(a, b),
            F::Div => self.builder.ins().fdiv(a, b),
            F::Rem => self.float_remainder(a, b),
            F::Min => self.builder.ins().fmin(a, b),
            F::Max => self.builder.ins().fmax(a, b),
        };
        self.define(results, value)
    }

    /// `a % b` with the sign of `a`, as `fmod` computes it. The remainder of
    /// two singles is exact in a double, so they widen to share the call.
    fn float_remainder(&mut self, a: Value, b: Value) -> Value {
        let single = self.builder.func.dfg.value_type(a) == types::F32;
        let (a, b) = if single {
            (
                self.builder.ins().fpromote(types::F64, a),
                self.builder.ins().fpromote(types::F64, b),
            )
        } else {
            (a, b)
        };
        let remainder = self.call_one(self.runtime.float_remainder, &[a, b]);
        if single {
            self.builder.ins().fdemote(types::F32, remainder)
        } else {
            remainder
        }
    }

    pub(super) fn float_unary(
        &mut self,
        results: &[Name],
        operator: FloatUnaryOperator,
        value: &Name,
    ) -> Result<(), EmitError> {
        use FloatUnaryOperator as F;
        let operand = self.value(value)?;
        let out = match operator {
            F::Neg => self.builder.ins().fneg(operand),
            F::Abs => self.builder.ins().fabs(operand),
            F::Sqrt => self.builder.ins().sqrt(operand),
            F::Floor => self.builder.ins().floor(operand),
            F::Ceil => self.builder.ins().ceil(operand),
            F::Trunc => self.builder.ins().trunc(operand),
            F::Round => self.builder.ins().nearest(operand),
        };
        self.define(results, out)
    }

    pub(super) fn convert(
        &mut self,
        results: &[Name],
        kind: ConvertKind,
        value: &Name,
        ty: &Type,
    ) -> Result<(), EmitError> {
        use ConvertKind as C;
        let source = self.ty_of(value)?.clone();
        let src = super::cl_type(&source)?;
        let dst = super::cl_type(ty)?;
        let operand = self.value(value)?;
        let out = match kind {
            // An `i1` is held as an `i8`, so widening one to `i8` (a `Bool`
            // written as a C byte) changes no machine type.
            C::Truncate | C::ZeroExtend | C::SignExtend if src == dst => operand,
            C::Truncate => self.builder.ins().ireduce(dst, operand),
            C::ZeroExtend => self.builder.ins().uextend(dst, operand),
            C::SignExtend => self.builder.ins().sextend(dst, operand),
            C::FloatTruncate => self.builder.ins().fdemote(dst, operand),
            C::FloatExtend => self.builder.ins().fpromote(dst, operand),
            C::SignedToFloat => self.builder.ins().fcvt_from_sint(dst, operand),
            C::UnsignedToFloat => self.builder.ins().fcvt_from_uint(dst, operand),
            C::PointerToInt | C::IntToPointer => {
                if src == dst {
                    operand
                } else if src.bits() < dst.bits() {
                    self.builder.ins().uextend(dst, operand)
                } else {
                    self.builder.ins().ireduce(dst, operand)
                }
            }
            C::Bitcast => {
                if src == dst {
                    operand
                } else {
                    self.builder
                        .ins()
                        .bitcast(dst, MemFlagsData::new(), operand)
                }
            }
            C::FloatToSigned | C::FloatToUnsigned => {
                self.float_to_int(kind, src, dst, operand, ty)?
            }
            C::FloatToSignedSaturating => self.builder.ins().fcvt_to_sint_sat(dst, operand),
            C::FloatToUnsignedSaturating => self.builder.ins().fcvt_to_uint_sat(dst, operand),
        };
        self.define(results, out)
    }

    #[allow(clippy::cast_precision_loss)]
    fn float_to_int(
        &mut self,
        kind: ConvertKind,
        src: types::Type,
        dst: types::Type,
        operand: Value,
        ty: &Type,
    ) -> Result<Value, EmitError> {
        let signed = matches!(kind, ConvertKind::FloatToSigned);
        let bits = scalar_bits(ty)?;
        let truncated = self.builder.ins().trunc(operand);
        let nan = self
            .builder
            .ins()
            .fcmp(FloatCC::Unordered, operand, operand);
        let (min_f, max_f) = if signed {
            let (min, max) = signed_bounds(bits);
            (min as f64, max as f64)
        } else {
            let max = if bits >= 128 {
                u128::MAX
            } else {
                (1u128 << bits) - 1
            };
            (0.0, max as f64)
        };
        let (min_c, max_c) = self.float_bounds(src, min_f, max_f);
        let lo = self.builder.ins().fcmp(FloatCC::LessThan, truncated, min_c);
        let hi = self
            .builder
            .ins()
            .fcmp(FloatCC::GreaterThan, truncated, max_c);
        let bad0 = self.builder.ins().bor(nan, lo);
        let bad = self.builder.ins().bor(bad0, hi);
        self.guard_trap(bad, 11);
        Ok(if signed {
            self.builder.ins().fcvt_to_sint(dst, operand)
        } else {
            self.builder.ins().fcvt_to_uint(dst, operand)
        })
    }

    fn float_bounds(&mut self, src: types::Type, min_f: f64, max_f: f64) -> (Value, Value) {
        if src == types::F32 {
            #[allow(clippy::cast_possible_truncation)]
            let (min, max) = (min_f as f32, max_f as f32);
            (
                self.builder.ins().f32const(min),
                self.builder.ins().f32const(max),
            )
        } else {
            (
                self.builder.ins().f64const(min_f),
                self.builder.ins().f64const(max_f),
            )
        }
    }

    pub(super) fn widen_to_i64(&mut self, name: &Name) -> Result<Value, EmitError> {
        let ty = self.ty_of(name)?.clone();
        let operand = self.value(name)?;
        Ok(match ty {
            Type::Integer(64)
            | Type::Pointer
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address(_)
            | Type::Function(_) => operand,
            Type::Integer(_) => self.builder.ins().uextend(types::I64, operand),
            Type::Float(64) => self
                .builder
                .ins()
                .bitcast(types::I64, MemFlagsData::new(), operand),
            Type::Float(32) => {
                let bits = self
                    .builder
                    .ins()
                    .bitcast(types::I32, MemFlagsData::new(), operand);
                self.builder.ins().uextend(types::I64, bits)
            }
            other => return Err(err(format!("cannot store `{other}`"))),
        })
    }
}

fn float_cc(predicate: FloatPredicate) -> FloatCC {
    use FloatPredicate as P;
    match predicate {
        P::Eq => FloatCC::Equal,
        P::Ne => FloatCC::OrderedNotEqual,
        P::Lt => FloatCC::LessThan,
        P::Le => FloatCC::LessThanOrEqual,
        P::Gt => FloatCC::GreaterThan,
        P::Ge => FloatCC::GreaterThanOrEqual,
        P::Ordered => FloatCC::Ordered,
        P::Unordered => FloatCC::Unordered,
        P::UnorderedEq => FloatCC::UnorderedOrEqual,
        P::UnorderedNe => FloatCC::NotEqual,
        P::UnorderedLt => FloatCC::UnorderedOrLessThan,
        P::UnorderedLe => FloatCC::UnorderedOrLessThanOrEqual,
        P::UnorderedGt => FloatCC::UnorderedOrGreaterThan,
        P::UnorderedGe => FloatCC::UnorderedOrGreaterThanOrEqual,
    }
}

fn int_cc(predicate: IntegerPredicate) -> IntCC {
    use IntegerPredicate as P;
    match predicate {
        P::Eq => IntCC::Equal,
        P::Ne => IntCC::NotEqual,
        P::SignedLt => IntCC::SignedLessThan,
        P::SignedLe => IntCC::SignedLessThanOrEqual,
        P::SignedGt => IntCC::SignedGreaterThan,
        P::SignedGe => IntCC::SignedGreaterThanOrEqual,
        P::UnsignedLt => IntCC::UnsignedLessThan,
        P::UnsignedLe => IntCC::UnsignedLessThanOrEqual,
        P::UnsignedGt => IntCC::UnsignedGreaterThan,
        P::UnsignedGe => IntCC::UnsignedGreaterThanOrEqual,
    }
}
