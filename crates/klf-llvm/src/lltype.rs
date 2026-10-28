//! Value typing over SSA values and the KIR-to-LLVM type mapping.

use crate::EmitError;
use klf_core::Name;
use klf_kir::{Function, Module, Type, infer_value_types};

/// Types of every SSA value in a function, from the shared `klf-kir` inference.
pub struct ValueTypes {
    values: klf_kir::ValueTypes,
}

impl ValueTypes {
    pub fn require(&self, name: &Name) -> Result<&Type, EmitError> {
        self.values
            .get(name)
            .ok_or_else(|| EmitError(format!("cannot type `%{name}`")))
    }

    #[must_use]
    pub fn infer(function: &Function, module: &Module) -> Self {
        Self {
            values: infer_value_types(function, module),
        }
    }
}

/// The LLVM type text for a KIR type in a value or result position.
pub fn llvm_type(ty: &Type) -> Result<String, EmitError> {
    Ok(match ty {
        Type::Integer(bits) => format!("i{bits}"),
        Type::Float(32) => "float".to_owned(),
        Type::Float(64) => "double".to_owned(),
        Type::Float(bits) => return Err(EmitError(format!("unsupported float width {bits}"))),
        Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_)
        | Type::Function(_) => "ptr".to_owned(),
        other => {
            return Err(EmitError(format!(
                "type not lowerable to LLVM yet: {other}"
            )));
        }
    })
}

/// The LLVM type text for a function result (`void` for unit and never).
pub fn llvm_result_type(ty: &Type) -> Result<String, EmitError> {
    match ty {
        Type::Unit | Type::Never => Ok("void".to_owned()),
        other => llvm_type(other),
    }
}

/// Integer bit width of a scalar type, for load/store and arithmetic.
pub fn scalar_bits(ty: &Type) -> Result<u32, EmitError> {
    Ok(match ty {
        Type::Integer(bits) | Type::Float(bits) => u32::from(*bits),
        Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_)
        | Type::Function(_) => 64,
        other => return Err(EmitError(format!("cannot size `{other}`"))),
    })
}

/// Byte size and alignment of an allocatable scalar type.
pub fn size_align(ty: &Type) -> Result<(u32, u32), EmitError> {
    let bytes = match ty {
        Type::Integer(bits) | Type::Float(bits) => u32::from(bits.div_ceil(8)),
        Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_)
        | Type::Function(_) => 8,
        other => return Err(EmitError(format!("cannot size `{other}` for allocation"))),
    };
    Ok((bytes.max(1), bytes.max(1)))
}
