//! Constants, integer and floating-point arithmetic, comparisons and casts.

use super::FnEmitter;
use crate::EmitError;
use crate::lltype::{llvm_type, scalar_bits};
use klf_core::Name;
use klf_kir::{
    BinaryOperator, ConvertKind, FloatBinaryOperator, FloatPredicate, FloatUnaryOperator,
    IntegerPredicate, Operation, Overflow, Type,
};

impl FnEmitter<'_, '_> {
    pub(super) fn const_string(&mut self, results: &[Name], value: &str) -> Result<(), EmitError> {
        let symbol = self
            .strings
            .get(value)
            .ok_or_else(|| EmitError(format!("missing const.string declaration for `{value}`")))?;
        let address = self.fresh();
        self.line(&format!(
            "  {address} = getelementptr inbounds i8, ptr @\"{symbol}\", i64 0"
        ));
        self.record_pair(results, address, value.len().to_string())
    }

    /// The constants and the overflow-reporting arithmetic.
    pub(super) fn scalar(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), EmitError>> {
        Some(match operation {
            Operation::ConstInt { ty, value } => self.const_int(results, ty, *value),
            Operation::ConstBool(value) => {
                self.const_int(results, &Type::Integer(1), i128::from(*value))
            }
            Operation::ConstNull => self.const_null(results),
            Operation::ConstZero(ty) => self.const_zero(results, ty),
            Operation::ConstFloat { ty, literal } => self.const_float(results, ty, literal),
            Operation::ConstString(value) => self.const_string(results, value),
            Operation::BinaryOverflow {
                operator,
                signed,
                lhs,
                rhs,
            } => self.binary_overflow(results, *operator, *signed, lhs, rhs),
            _ => return None,
        })
    }

    pub(super) fn const_int(
        &mut self,
        results: &[Name],
        ty: &Type,
        value: i128,
    ) -> Result<(), EmitError> {
        let _ = llvm_type(ty)?;
        let result = super::single(results)?.clone();
        self.record(&result, value.to_string());
        Ok(())
    }

    pub(super) fn const_null(&mut self, results: &[Name]) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        self.record(&result, "null".to_owned());
        Ok(())
    }

    pub(super) fn const_zero(&mut self, results: &[Name], ty: &Type) -> Result<(), EmitError> {
        let operand = match ty {
            Type::Integer(_) => "0".to_owned(),
            Type::Float(_) => "0.0".to_owned(),
            Type::Pointer
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address(_) => "null".to_owned(),
            other => return Err(EmitError(format!("cannot zero `{other}`"))),
        };
        let result = super::single(results)?.clone();
        self.record(&result, operand);
        Ok(())
    }

    pub(super) fn const_float(
        &mut self,
        results: &[Name],
        ty: &Type,
        literal: &str,
    ) -> Result<(), EmitError> {
        let bits = match ty {
            Type::Float(32) => {
                let value: f32 = literal
                    .parse()
                    .map_err(|_| EmitError(format!("bad float literal `{literal}`")))?;
                f64::from(value).to_bits()
            }
            Type::Float(64) => literal
                .parse::<f64>()
                .map_err(|_| EmitError(format!("bad float literal `{literal}`")))?
                .to_bits(),
            other => return Err(EmitError(format!("unsupported float type `{other}`"))),
        };
        let result = super::single(results)?.clone();
        self.record(&result, format!("0x{bits:016X}"));
        Ok(())
    }

    pub(super) fn alias(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let operand = self.value(value)?;
        let result = super::single(results)?.clone();
        self.record(&result, operand);
        Ok(())
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
        let bits = scalar_bits(&ty)?;
        let llty = llvm_type(&ty)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let result = super::single(results)?.clone();
        let operand = match operator {
            B::Add | B::Sub | B::Mul => {
                let op = match operator {
                    B::Add => "add",
                    B::Sub => "sub",
                    _ => "mul",
                };
                match overflow {
                    Overflow::Wrap => self.simple(op, &llty, &a, &b),
                    Overflow::Trap => self.trap_arith(op, signed, &llty, bits, &a, &b),
                    Overflow::Saturate => self.saturate_arith(op, signed, &llty, bits, &a, &b),
                }
            }
            B::And => self.simple("and", &llty, &a, &b),
            B::Or => self.simple("or", &llty, &a, &b),
            B::Xor => self.simple("xor", &llty, &a, &b),
            B::UnsignedDiv => self.guarded_div("udiv", &llty, &a, &b, overflow, bits),
            B::UnsignedRem => self.guarded_div("urem", &llty, &a, &b, overflow, bits),
            B::SignedDiv => self.guarded_div("sdiv", &llty, &a, &b, overflow, bits),
            B::SignedRem => self.guarded_div("srem", &llty, &a, &b, overflow, bits),
            B::ShiftLeft => self.shift("shl", &llty, &a, &b, bits, overflow),
            B::LogicalShiftRight => self.shift("lshr", &llty, &a, &b, bits, overflow),
            B::ArithmeticShiftRight => self.shift("ashr", &llty, &a, &b, bits, overflow),
            B::RotateLeft => self.rotate("fshl", bits, &a, &b),
            B::RotateRight => self.rotate("fshr", bits, &a, &b),
        };
        self.record(&result, operand);
        Ok(())
    }

    /// A rotate is a funnel shift with both halves taken from the same value.
    fn rotate(&mut self, op: &str, bits: u32, a: &str, b: &str) -> String {
        self.intrinsic(format!(
            "declare i{bits} @llvm.{op}.i{bits}(i{bits}, i{bits}, i{bits})"
        ));
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call i{bits} @llvm.{op}.i{bits}(i{bits} {a}, i{bits} {a}, i{bits} {b})"
        ));
        dest
    }

    fn simple(&mut self, op: &str, llty: &str, a: &str, b: &str) -> String {
        let dest = self.fresh();
        self.line(&format!("  {dest} = {op} {llty} {a}, {b}"));
        dest
    }

    fn trap_arith(
        &mut self,
        op: &str,
        signed: bool,
        llty: &str,
        bits: u32,
        a: &str,
        b: &str,
    ) -> String {
        let (value, flag) = self.overflowing(op, signed, llty, bits, a, b);
        let (ov, cont) = (self.fresh_label("ov"), self.fresh_label("ok"));
        self.line(&format!("  br i1 {flag}, label %{ov}, label %{cont}"));
        self.line(&format!("{ov}:"));
        self.trap(2);
        self.line(&format!("{cont}:"));
        value
    }

    /// `add.ovf`, `sub.ovf` and `mul.ovf`: the wrapped result and whether
    /// it overflowed.
    pub(super) fn binary_overflow(
        &mut self,
        results: &[Name],
        operator: BinaryOperator,
        signed: bool,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let op = match operator {
            BinaryOperator::Add => "add",
            BinaryOperator::Sub => "sub",
            BinaryOperator::Mul => "mul",
            other => return Err(EmitError(format!("`{other:?}.ovf` has no overflow form"))),
        };
        let ty = self.ty_of(lhs)?.clone();
        let bits = scalar_bits(&ty)?;
        let llty = llvm_type(&ty)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let (value, flag) = self.overflowing(op, signed, &llty, bits, &a, &b);
        self.record_pair(results, value, flag)
    }

    fn overflowing(
        &mut self,
        op: &str,
        signed: bool,
        llty: &str,
        bits: u32,
        a: &str,
        b: &str,
    ) -> (String, String) {
        let prefix = if signed { 's' } else { 'u' };
        let sop = format!("{prefix}{op}");
        self.intrinsic(format!(
            "declare {{ i{bits}, i1 }} @llvm.{sop}.with.overflow.i{bits}(i{bits}, i{bits})"
        ));
        let pair = self.fresh();
        self.line(&format!(
            "  {pair} = call {{ {llty}, i1 }} @llvm.{sop}.with.overflow.i{bits}({llty} {a}, {llty} {b})"
        ));
        let value = self.fresh();
        self.line(&format!(
            "  {value} = extractvalue {{ {llty}, i1 }} {pair}, 0"
        ));
        let flag = self.fresh();
        self.line(&format!(
            "  {flag} = extractvalue {{ {llty}, i1 }} {pair}, 1"
        ));
        (value, flag)
    }

    fn saturate_arith(
        &mut self,
        op: &str,
        signed: bool,
        llty: &str,
        bits: u32,
        a: &str,
        b: &str,
    ) -> String {
        if op == "mul" {
            return self.saturate_mul(signed, llty, bits, a, b);
        }
        let prefix = if signed { 's' } else { 'u' };
        let sop = format!("{prefix}{op}");
        self.intrinsic(format!(
            "declare i{bits} @llvm.{sop}.sat.i{bits}(i{bits}, i{bits})"
        ));
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call {llty} @llvm.{sop}.sat.i{bits}({llty} {a}, {llty} {b})"
        ));
        dest
    }

    fn saturate_mul(&mut self, signed: bool, llty: &str, bits: u32, a: &str, b: &str) -> String {
        let prefix = if signed { 's' } else { 'u' };
        self.intrinsic(format!(
            "declare {{ i{bits}, i1 }} @llvm.{prefix}mul.with.overflow.i{bits}(i{bits}, i{bits})"
        ));
        let (min, max) = if signed {
            signed_bounds(bits)
        } else {
            (
                0,
                i128::try_from(unsigned_max(bits)).expect("width <= 64 fits i128"),
            )
        };
        let pair = self.fresh();
        self.line(&format!(
            "  {pair} = call {{ {llty}, i1 }} @llvm.{prefix}mul.with.overflow.i{bits}({llty} {a}, {llty} {b})"
        ));
        let value = self.fresh();
        self.line(&format!(
            "  {value} = extractvalue {{ {llty}, i1 }} {pair}, 0"
        ));
        let flag = self.fresh();
        self.line(&format!(
            "  {flag} = extractvalue {{ {llty}, i1 }} {pair}, 1"
        ));
        // On overflow, clamp: unsigned can only overflow high (→ max); signed
        // clamps to MIN or MAX by the sign the true product would have had.
        let clamp = if signed {
            let sign = self.fresh();
            self.line(&format!("  {sign} = xor {llty} {a}, {b}"));
            let negative = self.fresh();
            self.line(&format!("  {negative} = icmp slt {llty} {sign}, 0"));
            let clamp = self.fresh();
            self.line(&format!(
                "  {clamp} = select i1 {negative}, {llty} {min}, {llty} {max}"
            ));
            clamp
        } else {
            let _ = min;
            max.to_string()
        };
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = select i1 {flag}, {llty} {clamp}, {llty} {value}"
        ));
        dest
    }

    fn guarded_div(
        &mut self,
        op: &str,
        llty: &str,
        a: &str,
        b: &str,
        overflow: Overflow,
        bits: u32,
    ) -> String {
        let signed = op.starts_with('s');
        let zero = self.fresh();
        self.line(&format!("  {zero} = icmp eq {llty} {b}, 0"));
        let (zt, zk) = (self.fresh_label("dz"), self.fresh_label("dk"));
        self.line(&format!("  br i1 {zero}, label %{zt}, label %{zk}"));
        self.line(&format!("{zt}:"));
        self.trap(10);
        self.line(&format!("{zk}:"));
        if !signed {
            return self.simple(op, llty, a, b);
        }
        let (min, _) = signed_bounds(bits);
        let is_min = self.fresh();
        self.line(&format!("  {is_min} = icmp eq {llty} {a}, {min}"));
        let is_neg1 = self.fresh();
        self.line(&format!("  {is_neg1} = icmp eq {llty} {b}, -1"));
        let ov = self.fresh();
        self.line(&format!("  {ov} = and i1 {is_min}, {is_neg1}"));
        if matches!(overflow, Overflow::Trap) {
            let (ot, ok) = (self.fresh_label("ov"), self.fresh_label("ok"));
            self.line(&format!("  br i1 {ov}, label %{ot}, label %{ok}"));
            self.line(&format!("{ot}:"));
            self.trap(2);
            self.line(&format!("{ok}:"));
            return self.simple(op, llty, a, b);
        }
        let (sp, nm, mg) = (
            self.fresh_label("sp"),
            self.fresh_label("nm"),
            self.fresh_label("mg"),
        );
        self.line(&format!("  br i1 {ov}, label %{sp}, label %{nm}"));
        self.line(&format!("{sp}:"));
        self.line(&format!("  br label %{mg}"));
        self.line(&format!("{nm}:"));
        let normal = self.fresh();
        self.line(&format!("  {normal} = {op} {llty} {a}, {b}"));
        self.line(&format!("  br label %{mg}"));
        self.line(&format!("{mg}:"));
        let special = if op == "sdiv" {
            min.to_string()
        } else {
            "0".to_owned()
        };
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = phi {llty} [ {special}, %{sp} ], [ {normal}, %{nm} ]"
        ));
        dest
    }

    fn shift(
        &mut self,
        op: &str,
        llty: &str,
        a: &str,
        b: &str,
        bits: u32,
        overflow: Overflow,
    ) -> String {
        if matches!(overflow, Overflow::Trap) {
            let oob = self.fresh();
            self.line(&format!("  {oob} = icmp uge {llty} {b}, {bits}"));
            let (ot, ok) = (self.fresh_label("shov"), self.fresh_label("shok"));
            self.line(&format!("  br i1 {oob}, label %{ot}, label %{ok}"));
            self.line(&format!("{ot}:"));
            self.trap(2);
            self.line(&format!("{ok}:"));
            return self.simple(op, llty, a, b);
        }
        let amount = self.fresh();
        self.line(&format!("  {amount} = urem {llty} {b}, {bits}"));
        let dest = self.fresh();
        self.line(&format!("  {dest} = {op} {llty} {a}, {amount}"));
        dest
    }

    pub(super) fn icmp(
        &mut self,
        results: &[Name],
        predicate: IntegerPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let llty = llvm_type(self.ty_of(lhs)?)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = icmp {} {llty} {a}, {b}",
            predicate_code(predicate)
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn fcmp(
        &mut self,
        results: &[Name],
        predicate: FloatPredicate,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        let llty = llvm_type(self.ty_of(lhs)?)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = fcmp {} {llty} {a}, {b}",
            float_predicate_code(predicate)
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn float_binary(
        &mut self,
        results: &[Name],
        operator: FloatBinaryOperator,
        lhs: &Name,
        rhs: &Name,
    ) -> Result<(), EmitError> {
        use FloatBinaryOperator as F;
        let ty = self.ty_of(lhs)?.clone();
        let llty = llvm_type(&ty)?;
        let a = self.value(lhs)?;
        let b = self.value(rhs)?;
        let dest = self.fresh();
        match operator {
            F::Add | F::Sub | F::Mul | F::Div | F::Rem => {
                let op = match operator {
                    F::Add => "fadd",
                    F::Sub => "fsub",
                    F::Mul => "fmul",
                    F::Div => "fdiv",
                    _ => "frem",
                };
                self.line(&format!("  {dest} = {op} {llty} {a}, {b}"));
            }
            F::Min | F::Max => {
                let name = if matches!(operator, F::Min) {
                    "minnum"
                } else {
                    "maxnum"
                };
                self.intrinsic(format!(
                    "declare {llty} @llvm.{name}.{llty}({llty}, {llty})"
                ));
                self.line(&format!(
                    "  {dest} = call {llty} @llvm.{name}.{llty}({llty} {a}, {llty} {b})"
                ));
            }
        }
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn float_unary(
        &mut self,
        results: &[Name],
        operator: FloatUnaryOperator,
        value: &Name,
    ) -> Result<(), EmitError> {
        use FloatUnaryOperator as F;
        let llty = llvm_type(self.ty_of(value)?)?;
        let operand = self.value(value)?;
        let dest = self.fresh();
        if matches!(operator, F::Neg) {
            self.line(&format!("  {dest} = fneg {llty} {operand}"));
        } else {
            let name = match operator {
                F::Abs => "fabs",
                F::Sqrt => "sqrt",
                F::Floor => "floor",
                F::Ceil => "ceil",
                F::Trunc => "trunc",
                F::Round => "roundeven",
                F::Neg => unreachable!(),
            };
            self.intrinsic(format!("declare {llty} @llvm.{name}.{llty}({llty})"));
            self.line(&format!(
                "  {dest} = call {llty} @llvm.{name}.{llty}({llty} {operand})"
            ));
        }
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn convert(
        &mut self,
        results: &[Name],
        kind: ConvertKind,
        value: &Name,
        ty: &Type,
    ) -> Result<(), EmitError> {
        use ConvertKind as C;
        let src = llvm_type(self.ty_of(value)?)?;
        let dst = llvm_type(ty)?;
        let operand = self.value(value)?;
        let result = super::single(results)?.clone();
        let simple = |kind: C| match kind {
            C::Truncate => Some("trunc"),
            C::ZeroExtend => Some("zext"),
            C::SignExtend => Some("sext"),
            C::FloatTruncate => Some("fptrunc"),
            C::FloatExtend => Some("fpext"),
            C::SignedToFloat => Some("sitofp"),
            C::UnsignedToFloat => Some("uitofp"),
            C::PointerToInt => Some("ptrtoint"),
            C::IntToPointer => Some("inttoptr"),
            _ => None,
        };
        if let Some(op) = simple(kind) {
            let casted = self.fresh();
            self.line(&format!("  {casted} = {op} {src} {operand} to {dst}"));
            self.record(&result, casted);
            return Ok(());
        }
        match kind {
            C::Bitcast => {
                if src == dst {
                    self.record(&result, operand);
                } else {
                    let casted = self.fresh();
                    self.line(&format!("  {casted} = bitcast {src} {operand} to {dst}"));
                    self.record(&result, casted);
                }
                Ok(())
            }
            C::FloatToSigned | C::FloatToUnsigned => {
                let casted = self.float_to_int(kind, &src, &dst, &operand, ty)?;
                self.record(&result, casted);
                Ok(())
            }
            _ => unreachable!(),
        }
    }

    // The bound casts to `f64` mirror the VM's range check (numeric.rs): the
    // comparison must be against the float value of the integer bound, precision
    // loss and all, or native and VM would trap on different inputs.
    #[allow(clippy::cast_precision_loss)]
    fn float_to_int(
        &mut self,
        kind: ConvertKind,
        src: &str,
        dst: &str,
        operand: &str,
        ty: &Type,
    ) -> Result<String, EmitError> {
        let signed = matches!(kind, ConvertKind::FloatToSigned);
        let dbits = scalar_bits(ty)?;
        self.intrinsic(format!("declare {src} @llvm.trunc.{src}({src})"));
        let truncated = self.fresh();
        self.line(&format!(
            "  {truncated} = call {src} @llvm.trunc.{src}({src} {operand})"
        ));
        let nan = self.fresh();
        self.line(&format!("  {nan} = fcmp uno {src} {operand}, {operand}"));
        let (min_f, max_f) = if signed {
            let (min, max) = signed_bounds(dbits);
            (min as f64, max as f64)
        } else {
            (0.0, unsigned_max(dbits) as f64)
        };
        let low = self.fresh();
        self.line(&format!(
            "  {low} = fcmp olt {src} {truncated}, {}",
            float_hex(min_f)
        ));
        let high = self.fresh();
        self.line(&format!(
            "  {high} = fcmp ogt {src} {truncated}, {}",
            float_hex(max_f)
        ));
        let bad0 = self.fresh();
        self.line(&format!("  {bad0} = or i1 {nan}, {low}"));
        let bad = self.fresh();
        self.line(&format!("  {bad} = or i1 {bad0}, {high}"));
        let (bt, bk) = (self.fresh_label("f2iov"), self.fresh_label("f2iok"));
        self.line(&format!("  br i1 {bad}, label %{bt}, label %{bk}"));
        self.line(&format!("{bt}:"));
        self.trap(11);
        self.line(&format!("{bk}:"));
        let op = if signed { "fptosi" } else { "fptoui" };
        let casted = self.fresh();
        self.line(&format!("  {casted} = {op} {src} {operand} to {dst}"));
        Ok(casted)
    }
}

fn signed_bounds(bits: u32) -> (i128, i128) {
    let min = -(1i128 << (bits - 1));
    let max = (1i128 << (bits - 1)) - 1;
    (min, max)
}

fn unsigned_max(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn float_hex(value: f64) -> String {
    format!("0x{:016X}", value.to_bits())
}

fn float_predicate_code(predicate: FloatPredicate) -> &'static str {
    use FloatPredicate as P;
    match predicate {
        P::Eq => "oeq",
        P::Ne => "one",
        P::Lt => "olt",
        P::Le => "ole",
        P::Gt => "ogt",
        P::Ge => "oge",
        P::Ordered => "ord",
        P::Unordered => "uno",
        P::UnorderedEq => "ueq",
        P::UnorderedNe => "une",
        P::UnorderedLt => "ult",
        P::UnorderedLe => "ule",
        P::UnorderedGt => "ugt",
        P::UnorderedGe => "uge",
    }
}

fn predicate_code(predicate: IntegerPredicate) -> &'static str {
    use IntegerPredicate as P;
    match predicate {
        P::Eq => "eq",
        P::Ne => "ne",
        P::SignedLt => "slt",
        P::SignedLe => "sle",
        P::SignedGt => "sgt",
        P::SignedGe => "sge",
        P::UnsignedLt => "ult",
        P::UnsignedLe => "ule",
        P::UnsignedGt => "ugt",
        P::UnsignedGe => "uge",
    }
}
