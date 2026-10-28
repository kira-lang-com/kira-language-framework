//! Lowering of constants, arithmetic, comparisons and conversions.

use super::{
    LowerError, Lowerer, err, float_predicate_code, integer_predicate_code, width_of_type,
};
use crate::bytecode::{Instr, Opcode, OverflowMode, WidthCode, arith_flags};
use crate::program::Const;
use crate::types::{integer_width, is_float64};
use klf_core::Name;
use klf_kir::{
    BinaryOperator, ConvertKind, FloatBinaryOperator, FloatPredicate, FloatUnaryOperator,
    IntegerPredicate, Overflow, Type,
};

impl Lowerer<'_> {
    pub(super) fn lower_const_string(
        &mut self,
        results: &[Name],
        value: &str,
    ) -> Result<(), LowerError> {
        let [address, length] = results else {
            return Err(err("const.string needs address and length results"));
        };
        let index = *self
            .string_globals
            .get(value)
            .ok_or_else(|| err("missing const.string global"))?;
        let index = i32::try_from(index).map_err(|_| err("too many globals"))?;
        self.emit(Instr::from_bx(
            Opcode::GlobalA,
            self.register(address)?,
            index,
        ));
        let length_register = self.register(length)?;
        let byte_length = i32::try_from(value.len())
            .map_err(|_| err("const.string length exceeds bytecode immediate"))?;
        self.emit(Instr::from_bx(Opcode::LoadI, length_register, byte_length));
        Ok(())
    }

    pub(super) fn lower_const_int(
        &mut self,
        results: &[Name],
        ty: &Type,
        value: i128,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let bits = integer_width(ty).ok_or_else(|| err("const.int needs integer type"))?;
        if bits > 64 {
            return Err(err("128-bit constants are not yet lowered"));
        }
        if let Ok(small) = i32::try_from(value) {
            self.emit(Instr::from_bx(Opcode::LoadI, dest, small));
        } else {
            let index = self.pool(Const::Int(value))?;
            self.emit(Instr::from_bx(Opcode::LoadK, dest, index));
        }
        Ok(())
    }

    pub(super) fn lower_const_float(
        &mut self,
        results: &[Name],
        ty: &Type,
        literal: &str,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let bits = if is_float64(ty) {
            literal
                .parse::<f64>()
                .map_err(|_| err("invalid f64 literal"))?
                .to_bits()
        } else {
            u64::from(
                literal
                    .parse::<f32>()
                    .map_err(|_| err("invalid f32 literal"))?
                    .to_bits(),
            )
        };
        let index = self.pool(Const::Float(bits))?;
        self.emit(Instr::from_bx(Opcode::LoadK, dest, index));
        Ok(())
    }

    pub(super) fn lower_const_zero(
        &mut self,
        results: &[Name],
        ty: &Type,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        match ty {
            Type::Integer(bits) if *bits <= 64 => {
                self.emit(Instr::from_bx(Opcode::LoadI, dest, 0));
                Ok(())
            }
            Type::Float(_) => {
                let index = self.pool(Const::Float(0))?;
                self.emit(Instr::from_bx(Opcode::LoadK, dest, index));
                Ok(())
            }
            Type::Pointer | Type::Address(_) => {
                self.emit(Instr::abc(Opcode::LoadNull, dest, 0, 0));
                Ok(())
            }
            other => Err(err(format!("const.zero not lowered for {other}"))),
        }
    }

    pub(super) fn lower_binary(
        &mut self,
        results: &[Name],
        operator: BinaryOperator,
        overflow: Overflow,
        signed_flag: bool,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let a = self.register(lhs)?;
        let b = self.register(rhs)?;
        let width = self.width_of(lhs)?;
        let signed = signed_flag
            || matches!(
                operator,
                BinaryOperator::SignedDiv
                    | BinaryOperator::SignedRem
                    | BinaryOperator::ArithmeticShiftRight
            );
        let mode = match overflow {
            Overflow::Wrap => OverflowMode::Wrap,
            Overflow::Trap => OverflowMode::Trap,
            Overflow::Saturate => OverflowMode::Saturate,
        };
        let opcode = match operator {
            BinaryOperator::Add => Opcode::Add,
            BinaryOperator::Sub => Opcode::Sub,
            BinaryOperator::Mul => Opcode::Mul,
            BinaryOperator::SignedDiv => Opcode::SDiv,
            BinaryOperator::UnsignedDiv => Opcode::UDiv,
            BinaryOperator::SignedRem => Opcode::SRem,
            BinaryOperator::UnsignedRem => Opcode::URem,
            BinaryOperator::And => Opcode::And,
            BinaryOperator::Or => Opcode::Or,
            BinaryOperator::Xor => Opcode::Xor,
            BinaryOperator::RotateLeft => Opcode::RotL,
            BinaryOperator::RotateRight => Opcode::RotR,
            BinaryOperator::ShiftLeft => Opcode::Shl,
            BinaryOperator::LogicalShiftRight => Opcode::LShr,
            BinaryOperator::ArithmeticShiftRight => Opcode::AShr,
        };
        self.emit(Instr::abc(opcode, dest, a, b).with_flags(arith_flags(width, signed, mode)));
        Ok(())
    }

    /// `add.ovf`, `sub.ovf` and `mul.ovf`: the wrapped result, and its
    /// overflow read back from it with the bytecode's own operations.
    pub(super) fn lower_binary_overflow(
        &mut self,
        results: &[Name],
        operator: BinaryOperator,
        signed: bool,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let [value, overflowed] = results else {
            return Err(err("an overflowing operation needs a value and a flag"));
        };
        let (dest, flag) = (self.register(value)?, self.register(overflowed)?);
        let (a, b) = (self.register(lhs)?, self.register(rhs)?);
        let width = self.width_of(lhs)?;
        let wrap = arith_flags(width, signed, OverflowMode::Wrap);
        let opcode = match operator {
            BinaryOperator::Add => Opcode::Add,
            BinaryOperator::Sub => Opcode::Sub,
            BinaryOperator::Mul => Opcode::Mul,
            other => return Err(err(format!("{other:?} has no overflowing form"))),
        };
        let result = self.fresh()?;
        self.emit(Instr::abc(opcode, result, a, b).with_flags(wrap));
        let compare = |predicate| width.code() | (integer_predicate_code(predicate) << 3);
        let bitwise = arith_flags(width, false, OverflowMode::Wrap);
        let zero = self.fresh()?;
        self.emit(Instr::from_bx(Opcode::LoadI, zero, 0));
        match (operator, signed) {
            (BinaryOperator::Add | BinaryOperator::Sub, true) => {
                let (left, right) = (self.fresh()?, self.fresh()?);
                let pairs = if operator == BinaryOperator::Add {
                    [(a, result), (b, result)]
                } else {
                    [(a, b), (a, result)]
                };
                for (into, (x, y)) in [left, right].into_iter().zip(pairs) {
                    self.emit(Instr::abc(Opcode::Xor, into, x, y).with_flags(bitwise));
                }
                self.emit(Instr::abc(Opcode::And, left, left, right).with_flags(bitwise));
                let less = compare(IntegerPredicate::SignedLt);
                self.emit(Instr::abc(Opcode::Icmp, flag, left, zero).with_flags(less));
            }
            (BinaryOperator::Add, false) => {
                let below = compare(IntegerPredicate::UnsignedLt);
                self.emit(Instr::abc(Opcode::Icmp, flag, result, a).with_flags(below));
            }
            (BinaryOperator::Sub, false) => {
                let below = compare(IntegerPredicate::UnsignedLt);
                self.emit(Instr::abc(Opcode::Icmp, flag, a, b).with_flags(below));
            }
            _ => self.mul_overflow(width, flag, [result, a, b, zero], signed)?,
        }
        self.emit(Instr::abc(Opcode::Mov, dest, result, 0));
        Ok(())
    }

    /// A product overflowed when dividing it by one operand does not give
    /// the other back. A zero operand never overflows; in a signed product
    /// by -1 only the minimum overflows, and it alone stays negative.
    fn mul_overflow(
        &mut self,
        width: WidthCode,
        flag: u16,
        [result, a, b, zero]: [u16; 4],
        signed: bool,
    ) -> Result<(), LowerError> {
        let compare = |predicate| width.code() | (integer_predicate_code(predicate) << 3);
        let bitwise = arith_flags(width, false, OverflowMode::Wrap);
        let none = self.fresh()?;
        self.emit(
            Instr::abc(Opcode::Icmp, none, b, zero).with_flags(compare(IntegerPredicate::Eq)),
        );
        let one = self.fresh()?;
        self.emit(Instr::from_bx(Opcode::LoadI, one, 1));
        let divisor = self.pick(none, one, b)?;
        let quotient = self.fresh()?;
        let divide = if signed { Opcode::SDiv } else { Opcode::UDiv };
        let wrap = arith_flags(width, signed, OverflowMode::Wrap);
        self.emit(Instr::abc(divide, quotient, result, divisor).with_flags(wrap));
        let differs = self.fresh()?;
        let ne = compare(IntegerPredicate::Ne);
        self.emit(Instr::abc(Opcode::Icmp, differs, quotient, a).with_flags(ne));
        let checked = if signed {
            let minus = self.fresh()?;
            self.emit(Instr::from_bx(Opcode::LoadI, minus, -1));
            let negating = self.fresh()?;
            let eq = compare(IntegerPredicate::Eq);
            self.emit(Instr::abc(Opcode::Icmp, negating, b, minus).with_flags(eq));
            let both = self.fresh()?;
            self.emit(Instr::abc(Opcode::And, both, a, result).with_flags(bitwise));
            let minimum = self.fresh()?;
            let less = compare(IntegerPredicate::SignedLt);
            self.emit(Instr::abc(Opcode::Icmp, minimum, both, zero).with_flags(less));
            self.pick(negating, minimum, differs)?
        } else {
            differs
        };
        let picked = self.pick(none, zero, checked)?;
        self.emit(Instr::abc(Opcode::Mov, flag, picked, 0));
        Ok(())
    }

    /// `yes` when `condition` holds, otherwise `no`.
    fn pick(&mut self, condition: u16, yes: u16, no: u16) -> Result<u16, LowerError> {
        let (first, second, picked) = (self.fresh()?, self.fresh()?, self.fresh()?);
        self.emit(Instr::abc(Opcode::Mov, first, yes, 0));
        self.emit(Instr::abc(Opcode::Mov, second, no, 0));
        self.emit(Instr::abc(Opcode::Select, picked, condition, first));
        Ok(picked)
    }

    pub(super) fn lower_icmp(
        &mut self,
        results: &[Name],
        predicate: IntegerPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let a = self.register(lhs)?;
        let b = self.register(rhs)?;
        let width = self.width_of(lhs)?;
        let code = integer_predicate_code(predicate);
        let flags = width.code() | (code << 3);
        self.emit(Instr::abc(Opcode::Icmp, dest, a, b).with_flags(flags));
        Ok(())
    }

    pub(super) fn lower_fcmp(
        &mut self,
        results: &[Name],
        predicate: FloatPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let a = self.register(lhs)?;
        let b = self.register(rhs)?;
        let is_64 = self.float_is_64(lhs)?;
        let code = float_predicate_code(predicate);
        let flags = u8::from(is_64) | (code << 1);
        self.emit(Instr::abc(Opcode::FCmp, dest, a, b).with_flags(flags));
        Ok(())
    }

    pub(super) fn lower_float_binary(
        &mut self,
        results: &[Name],
        operator: FloatBinaryOperator,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let a = self.register(lhs)?;
        let b = self.register(rhs)?;
        let flags = u8::from(self.float_is_64(lhs)?);
        let opcode = match operator {
            FloatBinaryOperator::Add => Opcode::FAdd,
            FloatBinaryOperator::Sub => Opcode::FSub,
            FloatBinaryOperator::Mul => Opcode::FMul,
            FloatBinaryOperator::Div => Opcode::FDiv,
            FloatBinaryOperator::Rem => Opcode::FRem,
            FloatBinaryOperator::Min => Opcode::FMin,
            FloatBinaryOperator::Max => Opcode::FMax,
        };
        self.emit(Instr::abc(opcode, dest, a, b).with_flags(flags));
        Ok(())
    }

    pub(super) fn lower_float_unary(
        &mut self,
        results: &[Name],
        operator: FloatUnaryOperator,
        value: &Name,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let source = self.register(value)?;
        let flags = u8::from(self.float_is_64(value)?);
        let opcode = match operator {
            FloatUnaryOperator::Neg => Opcode::FNeg,
            FloatUnaryOperator::Abs => Opcode::FAbs,
            FloatUnaryOperator::Sqrt => Opcode::FSqrt,
            FloatUnaryOperator::Floor => Opcode::FFloor,
            FloatUnaryOperator::Ceil => Opcode::FCeil,
            FloatUnaryOperator::Trunc => Opcode::FTrunc,
            FloatUnaryOperator::Round => Opcode::FRound,
        };
        self.emit(Instr::abc(opcode, dest, source, 0).with_flags(flags));
        Ok(())
    }

    pub(super) fn float_is_64(&self, name: &Name) -> Result<bool, LowerError> {
        let ty = self
            .types
            .get(name)
            .ok_or_else(|| err(format!("cannot type `%{name}`")))?;
        Ok(is_float64(ty))
    }

    pub(super) fn lower_convert(
        &mut self,
        results: &[Name],
        kind: ConvertKind,
        value: &Name,
        ty: &Type,
    ) -> Result<(), LowerError> {
        let dest = self.result(results)?;
        let source = self.register(value)?;
        match kind {
            ConvertKind::Truncate | ConvertKind::ZeroExtend | ConvertKind::SignExtend => {
                let source_width = self.width_of(value)?;
                let dest_width = width_of_type(ty)?;
                let opcode = match kind {
                    ConvertKind::Truncate => Opcode::Trunc,
                    ConvertKind::ZeroExtend => Opcode::ZExt,
                    _ => Opcode::SExt,
                };
                let flags = source_width.code() | (dest_width.code() << 3);
                self.emit(Instr::abc(opcode, dest, source, 0).with_flags(flags));
            }
            ConvertKind::SignedToFloat | ConvertKind::UnsignedToFloat => {
                let source_width = self.width_of(value)?;
                let opcode = if matches!(kind, ConvertKind::SignedToFloat) {
                    Opcode::SiToFp
                } else {
                    Opcode::UiToFp
                };
                let flags = source_width.code() | (u8::from(is_float64(ty)) << 3);
                self.emit(Instr::abc(opcode, dest, source, 0).with_flags(flags));
            }
            ConvertKind::FloatToSigned
            | ConvertKind::FloatToUnsigned
            | ConvertKind::FloatToSignedSaturating
            | ConvertKind::FloatToUnsignedSaturating => {
                let dest_width = width_of_type(ty)?;
                let (opcode, overflow) = match kind {
                    ConvertKind::FloatToSigned => (Opcode::FpToSi, OverflowMode::Trap),
                    ConvertKind::FloatToUnsigned => (Opcode::FpToUi, OverflowMode::Trap),
                    ConvertKind::FloatToSignedSaturating => {
                        (Opcode::FpToSi, OverflowMode::Saturate)
                    }
                    _ => (Opcode::FpToUi, OverflowMode::Saturate),
                };
                let flags = dest_width.code()
                    | (u8::from(self.float_is_64(value)?) << 3)
                    | (overflow.code() << 4);
                self.emit(Instr::abc(opcode, dest, source, 0).with_flags(flags));
            }
            ConvertKind::FloatTruncate => {
                self.emit(Instr::abc(Opcode::FpTrunc, dest, source, 0));
            }
            ConvertKind::FloatExtend => {
                self.emit(Instr::abc(Opcode::FpExt, dest, source, 0));
            }
            ConvertKind::Bitcast | ConvertKind::PointerToInt | ConvertKind::IntToPointer => {
                self.emit(Instr::abc(Opcode::Mov, dest, source, 0));
            }
        }
        Ok(())
    }
}
